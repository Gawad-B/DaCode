//! `dacode-render` — the GPU layer: rectangles and shaped text, nothing else.
//!
//! Owning `winit` + `wgpu` + `glyphon` directly rather than adopting a widget framework is a
//! deliberate cost. Immediate-mode and retained-widget toolkits are not built for per-glyph
//! styling over million-line documents with block cursors and virtualized scrolling, and the
//! ceiling they impose would be hit exactly where this project cannot afford one.
//!
//! The UI layer above builds a [`Frame`] of primitives with no GPU involvement, which keeps
//! layout testable headlessly and keeps this module small.

pub mod frame;
mod headless;
pub mod icon;
mod mark;
mod painter;
mod quad;
mod shape;
mod text;
mod timer;

pub use frame::{
    Align, Frame, Layer, MARK_INK_LEFT, Mark, Quad, Rect, Shape, ShapeKind, TextRun, Zoom,
};
pub use headless::Headless;
pub use icon::Icon;
pub use timer::FrameTimer;

use anyhow::{Context, Result};
use dacode_theme::Oklch;
use painter::Painter;
use std::sync::Arc;
use wgpu::CurrentSurfaceTexture;
use winit::window::Window;

pub struct Renderer {
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    painter: Painter,
    /// Set when the surface asked to be reconfigured while a frame was still in hand.
    ///
    /// It cannot be done at that moment. wgpu requires the surface texture to be **dropped before**
    /// the surface is configured, and configuring while holding one is a validation error — which
    /// the default handler turns into a panic, whose unwind drops the texture, whose drop is a
    /// second validation error, which panics again. A panic during a panic aborts.
    ///
    /// That was the whole mechanism behind "minimise DaCode and it closes": minimising is precisely
    /// what makes the surface start reporting `Suboptimal`, and the branch that handled it
    /// reconfigured with the texture still alive. It died on the first minimise, every time.
    reconfigure: bool,
    /// False while the window has no area — which on Windows means minimised.
    ///
    /// Rendering then is not just wasted: the config still holds the last real size, so every frame
    /// acquires, fails, and reconfigures a surface that has no size to be configured to. Skipping it
    /// entirely is both correct and the difference between an idle process and one spinning a GPU
    /// queue behind an icon in the taskbar.
    visible: bool,
    pub timer: FrameTimer,
}

/// The surface alpha mode for a window, given what the surface supports and whether it should let
/// the desktop through.
///
/// Premultiplied first, because that is what the blending produces: a background drawn at 85% over
/// a cleared canvas leaves `rgb * 0.85` beside an alpha of 0.85. Straight alpha is second, and is
/// given a clear colour that makes it come out nearly right (see [`clear_color`]). With neither, the
/// window stays opaque — the compositor would ignore the alpha anyway, and an X11 session with no
/// compositing manager is the common case for that.
pub fn alpha_mode(
    supported: &[wgpu::CompositeAlphaMode],
    translucent: bool,
) -> wgpu::CompositeAlphaMode {
    use wgpu::CompositeAlphaMode::{Opaque, PostMultiplied, PreMultiplied};
    let first = supported.first().copied().unwrap_or(Opaque);
    if !translucent {
        return if supported.contains(&Opaque) {
            Opaque
        } else {
            first
        };
    }
    [PreMultiplied, PostMultiplied]
        .into_iter()
        .find(|mode| supported.contains(mode))
        .unwrap_or(if supported.contains(&Opaque) {
            Opaque
        } else {
            first
        })
}

/// What the canvas is cleared to before a frame.
///
/// An opaque window clears to its background, as it always did, so startup never flashes. A
/// translucent one clears to nothing at all and lets the backdrops set the alpha: premultiplied to
/// black, since anything else would add light where nothing was drawn; straight-alpha to the
/// background's own colour with no opacity, so a backdrop blended over it keeps its hue.
pub fn clear_color(background: Oklch, mode: wgpu::CompositeAlphaMode) -> [f32; 4] {
    let [r, g, b, _] = background.to_linear();
    match mode {
        wgpu::CompositeAlphaMode::PreMultiplied => [0.0, 0.0, 0.0, 0.0],
        wgpu::CompositeAlphaMode::PostMultiplied => [r, g, b, 0.0],
        _ => [r, g, b, 1.0],
    }
}

impl Renderer {
    /// `translucent` asks for a surface the desktop can show through. Whether it got one is
    /// [`Renderer::is_translucent`]: not every platform and compositor can give it.
    pub fn new(window: Arc<Window>, translucent: bool) -> Result<Self> {
        pollster::block_on(Self::new_async(window, translucent))
    }

    /// Whether the window can actually show the desktop through it.
    pub fn is_translucent(&self) -> bool {
        !matches!(
            self.config.alpha_mode,
            wgpu::CompositeAlphaMode::Opaque | wgpu::CompositeAlphaMode::Auto
        )
    }

    async fn new_async(window: Arc<Window>, translucent: bool) -> Result<Self> {
        let size = window.inner_size();
        let (width, height) = (size.width.max(1), size.height.max(1));

        let instance = wgpu::Instance::default();
        let surface = instance
            .create_surface(window.clone())
            .context("creating the window surface")?;

        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                // An editor is not a game: integrated graphics is the better default for
                // battery life, and the workload is trivial for any GPU.
                power_preference: wgpu::PowerPreference::LowPower,
                compatible_surface: Some(&surface),
                ..Default::default()
            })
            .await
            .context("no suitable GPU adapter")?;

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("dacode device"),
                ..Default::default()
            })
            .await
            .context("requesting the GPU device")?;

        let mut config = surface
            .get_default_config(&adapter, width, height)
            .context("surface is incompatible with this adapter")?;
        // Vsync: an editor that renders faster than the display is burning battery for nothing.
        config.present_mode = wgpu::PresentMode::AutoVsync;
        config.alpha_mode =
            alpha_mode(&surface.get_capabilities(&adapter).alpha_modes, translucent);
        surface.configure(&device, &config);

        // A GPU error must not take the editor with it. wgpu's default handler panics, and a panic
        // on the UI thread of a Windows GUI build — which has no console — is a window that
        // vanishes with nothing said anywhere. Unsaved text is written to the recovery file every
        // couple of seconds and so survives, but a dropped frame is still an incomparably better
        // outcome than a lost session, and an editor that can be closed by a driver quirk is not
        // one anybody should trust their work to.
        device.on_uncaptured_error(Arc::new(|error| {
            eprintln!("dacode: gpu error, frame dropped: {error}");
        }));

        let format = config.format;
        Ok(Self {
            surface,
            config,
            painter: Painter::new(device, queue, format),
            reconfigure: false,
            visible: true,
            timer: FrameTimer::new(),
        })
    }

    /// Whether there is anything to draw into.
    pub fn is_visible(&self) -> bool {
        self.visible
    }

    pub fn size(&self) -> (u32, u32) {
        (self.config.width, self.config.height)
    }

    pub fn resolved_face(&self) -> &str {
        self.painter.resolved_face()
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        // A minimised window reports 0x0, and a surface cannot be configured to no size at all. The
        // last real size is kept, so restoring redraws at the size it had rather than at a default.
        self.visible = width > 0 && height > 0;
        if !self.visible {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.painter.device, &self.config);
        self.reconfigure = false;
    }

    /// Draws a frame. `clear` is painted before anything else; the design requires the window
    /// background to be set before the first paint so startup never flashes light.
    pub fn render(&mut self, frame: &Frame, clear: Oklch) -> Result<()> {
        // A minimised window has nothing to draw into, and trying costs a failed acquire and a
        // reconfigure per frame for as long as it stays minimised.
        if !self.visible {
            return Ok(());
        }

        // Whatever the last frame asked for, now that nothing is held.
        if std::mem::take(&mut self.reconfigure) {
            self.surface.configure(&self.painter.device, &self.config);
        }

        let surface_texture = match self.surface.get_current_texture() {
            CurrentSurfaceTexture::Success(texture) => texture,
            // Suboptimal still hands back a usable texture, so this frame is drawn rather than
            // dropped — but the reconfigure it is asking for has to wait for the next one, when the
            // texture is no longer in hand. See `reconfigure`: doing it here killed the process.
            CurrentSurfaceTexture::Suboptimal(texture) => {
                self.reconfigure = true;
                texture
            }
            // Routine, not errors: resize, monitor change, minimized window. Skip the frame, and
            // reconfigure at the top of the next one rather than here — the same rule as
            // `Suboptimal`, since nothing guarantees the acquire left nothing behind.
            CurrentSurfaceTexture::Outdated | CurrentSurfaceTexture::Lost => {
                self.reconfigure = true;
                self.timer.end();
                return Ok(());
            }
            CurrentSurfaceTexture::Timeout | CurrentSurfaceTexture::Occluded => {
                self.timer.end();
                return Ok(());
            }
            CurrentSurfaceTexture::Validation => {
                anyhow::bail!("surface validation error while acquiring a frame")
            }
        };

        let view = surface_texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let (width, height) = (self.config.width, self.config.height);

        let mut encoder =
            self.painter
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("dacode frame"),
                });

        let clear = if frame.transparency() > 0.0 {
            clear_color(clear, self.config.alpha_mode)
        } else {
            clear_color(clear, wgpu::CompositeAlphaMode::Opaque)
        };
        self.painter
            .paint(&mut encoder, &view, width, height, frame, clear)?;

        self.painter.queue.submit(Some(encoder.finish()));
        self.painter.queue.present(surface_texture);
        self.painter.trim_atlas();

        self.timer.end();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wgpu::CompositeAlphaMode::{Inherit, Opaque, PostMultiplied, PreMultiplied};

    #[test]
    fn a_translucent_window_takes_the_mode_its_blending_produces() {
        assert_eq!(alpha_mode(&[Opaque, PreMultiplied], true), PreMultiplied);
        assert_eq!(alpha_mode(&[Opaque, PostMultiplied], true), PostMultiplied);
        assert_eq!(
            alpha_mode(&[Opaque, Inherit], true),
            Opaque,
            "with no way to show through, the window stays opaque rather than guessing"
        );
    }

    #[test]
    fn an_opaque_window_never_asks_for_alpha() {
        // A surface told to blend with the desktop while every pixel claims to be opaque costs the
        // compositor work for nothing — and on some drivers shows garbage where alpha was undefined.
        assert_eq!(alpha_mode(&[PreMultiplied, Opaque], false), Opaque);
    }

    #[test]
    fn a_premultiplied_canvas_is_cleared_to_nothing() {
        let background = Oklch::new(0.3, 0.02, 250.0);
        assert_eq!(clear_color(background, PreMultiplied), [0.0; 4]);
        assert_eq!(clear_color(background, Opaque)[3], 1.0);
        assert_eq!(clear_color(background, PostMultiplied)[3], 0.0);
    }
}
