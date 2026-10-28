//! The mouse as the bytes a full-screen program asks for.
//!
//! A program that wants the mouse — `htop`, `vim` with `mouse=a`, `less --mouse`, `tmux` — turns on
//! one of xterm's reporting modes, and from then on a click in its window is its click and not the
//! terminal's selection. Pure functions over the modes alacritty's parser tracked, so every encoding
//! here is a unit test rather than something to check by hand in a program that may not be
//! installed.
//!
//! Two encodings, because programs ask for one or the other:
//!
//! - **SGR** (`CSI < b ; x ; y M`, `m` for a release), which every program written this century
//!   asks for. Coordinates are decimal, so a click in column 300 reports column 300, and a release
//!   says which button came up.
//! - **The X10/normal encoding** (`CSI M` and three bytes), the fallback for a program that did
//!   not ask for SGR. Each value is a byte offset by 32, so nothing past column 223 can be said at
//!   all — such a click is dropped rather than reported somewhere it was not — and a release cannot
//!   say which button it was. With `UTF8_MOUSE` the coordinates are UTF-8 characters instead, which
//!   reaches column 2015.

use alacritty_terminal::term::TermMode;

/// Which button, as xterm numbers them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
    /// One notch of the wheel away from you.
    WheelUp,
    /// One notch towards you.
    WheelDown,
}

/// What happened to the button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseAction {
    Press,
    Release,
    /// The pointer moved with the button held.
    Drag,
}

/// The modifiers held, which xterm folds into the button number.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MouseModifiers {
    pub shift: bool,
    pub alt: bool,
    pub ctrl: bool,
}

/// One mouse event, at a cell of the visible grid counted from zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MouseEvent {
    pub button: MouseButton,
    pub action: MouseAction,
    pub row: usize,
    pub column: usize,
    pub modifiers: MouseModifiers,
}

/// What the program asked for, read out of alacritty's [`TermMode`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MouseModes {
    /// `?1000`: presses and releases.
    pub click: bool,
    /// `?1002`: those, and movement while a button is held.
    pub drag: bool,
    /// `?1003`: every movement. Reported here as drags only — see [`MouseModes::wants`].
    pub motion: bool,
    /// `?1006`: the SGR encoding.
    pub sgr: bool,
    /// `?1005`: UTF-8 coordinates in the normal encoding.
    pub utf8: bool,
    /// The alternate screen is up: a full-screen program, with no history to scroll.
    pub alt_screen: bool,
    /// `?1007`: on the alternate screen, the wheel is sent as arrow keys.
    pub alternate_scroll: bool,
    /// DECCKM, which changes how those arrow keys are spelled.
    pub app_cursor: bool,
}

impl MouseModes {
    pub fn from_mode(mode: TermMode) -> Self {
        Self {
            click: mode.contains(TermMode::MOUSE_REPORT_CLICK),
            drag: mode.contains(TermMode::MOUSE_DRAG),
            motion: mode.contains(TermMode::MOUSE_MOTION),
            sgr: mode.contains(TermMode::SGR_MOUSE),
            utf8: mode.contains(TermMode::UTF8_MOUSE),
            alt_screen: mode.contains(TermMode::ALT_SCREEN),
            alternate_scroll: mode.contains(TermMode::ALTERNATE_SCROLL),
            app_cursor: mode.contains(TermMode::APP_CURSOR),
        }
    }

    /// Whether the program has the mouse at all. While it does, a plain click is the program's;
    /// shift held is the way to select anyway, which is xterm's convention and every terminal's
    /// since.
    pub fn reporting(&self) -> bool {
        self.click || self.drag || self.motion
    }

    /// Whether this event is one the program asked to be told about.
    ///
    /// Movement with no button held, which `?1003` also asks for, is not reported: nothing here
    /// tracks a hover over the grid, and sending it would cost a write per pointer event for the one
    /// mode almost nothing uses. A program in that mode still gets every press, drag and release.
    pub fn wants(&self, event: &MouseEvent) -> bool {
        match event.action {
            MouseAction::Press | MouseAction::Release => self.reporting(),
            MouseAction::Drag => self.drag || self.motion,
        }
    }
}

/// The bytes for `event`, or `None` when the program did not ask for it or it cannot be encoded.
pub fn encode(event: &MouseEvent, modes: &MouseModes) -> Option<Vec<u8>> {
    if !modes.wants(event) {
        return None;
    }
    let wheel = matches!(event.button, MouseButton::WheelUp | MouseButton::WheelDown);
    // A wheel has no release and no drag: a notch is a press and nothing else.
    if wheel && event.action != MouseAction::Press {
        return None;
    }
    let button: u32 = match event.button {
        MouseButton::Left => 0,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
        MouseButton::WheelUp => 64,
        MouseButton::WheelDown => 65,
    };
    let modifiers = 4 * u32::from(event.modifiers.shift)
        + 8 * u32::from(event.modifiers.alt)
        + 16 * u32::from(event.modifiers.ctrl);
    let motion = if event.action == MouseAction::Drag {
        32
    } else {
        0
    };
    let column = event.column as u32 + 1;
    let row = event.row as u32 + 1;

    if modes.sgr {
        let code = button + modifiers + motion;
        let last = if event.action == MouseAction::Release {
            'm'
        } else {
            'M'
        };
        return Some(format!("\x1b[<{code};{column};{row}{last}").into_bytes());
    }

    // The normal encoding cannot say which button came up: every release is button 3.
    let code = if event.action == MouseAction::Release {
        3
    } else {
        button
    } + modifiers
        + motion;
    let mut bytes = b"\x1b[M".to_vec();
    bytes.push((32 + code) as u8);
    for value in [column, row] {
        let value = 32 + value;
        if modes.utf8 {
            // Two bytes of UTF-8 reach 2047; past that it cannot be said.
            let c = char::from_u32(value).filter(|_| value < 2048)?;
            let mut buffer = [0; 4];
            bytes.extend_from_slice(c.encode_utf8(&mut buffer).as_bytes());
        } else if value <= 255 {
            bytes.push(value as u8);
        } else {
            return None;
        }
    }
    Some(bytes)
}

/// The wheel on the alternate screen of a program that did not ask for the mouse: `lines` arrow
/// keys, up for a positive count, spelled for DECCKM. Capped, so a fast flick on a touchpad does
/// not send a pager a thousand keys.
pub fn alternate_scroll(lines: i32, app_cursor: bool) -> Vec<u8> {
    let arrow: &[u8] = match (lines > 0, app_cursor) {
        (true, true) => b"\x1bOA",
        (true, false) => b"\x1b[A",
        (false, true) => b"\x1bOB",
        (false, false) => b"\x1b[B",
    };
    arrow.repeat(lines.unsigned_abs().min(64) as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(button: MouseButton, action: MouseAction, row: usize, column: usize) -> MouseEvent {
        MouseEvent {
            button,
            action,
            row,
            column,
            modifiers: MouseModifiers::default(),
        }
    }

    fn sgr() -> MouseModes {
        MouseModes {
            click: true,
            sgr: true,
            ..MouseModes::default()
        }
    }

    #[test]
    fn nothing_is_reported_to_a_program_that_did_not_ask() {
        // Without a reporting mode a click is the terminal's own — a selection — and sending the
        // shell `ESC [ M` would print garbage at the prompt.
        let click = event(MouseButton::Left, MouseAction::Press, 0, 0);
        assert_eq!(encode(&click, &MouseModes::default()), None);
        assert!(!MouseModes::default().reporting());
    }

    #[test]
    fn sgr_reports_one_based_decimal_cells_and_says_which_button_came_up() {
        let modes = sgr();
        let press = event(MouseButton::Left, MouseAction::Press, 4, 9);
        assert_eq!(encode(&press, &modes).unwrap(), b"\x1b[<0;10;5M");
        let release = event(MouseButton::Right, MouseAction::Release, 4, 9);
        assert_eq!(encode(&release, &modes).unwrap(), b"\x1b[<2;10;5m");
        // Past the old 223-column ceiling, which is the reason SGR exists.
        let far = event(MouseButton::Middle, MouseAction::Press, 0, 400);
        assert_eq!(encode(&far, &modes).unwrap(), b"\x1b[<1;401;1M");
    }

    #[test]
    fn modifiers_and_dragging_are_folded_into_the_button() {
        let modes = MouseModes {
            drag: true,
            sgr: true,
            ..MouseModes::default()
        };
        let mut drag = event(MouseButton::Left, MouseAction::Drag, 0, 0);
        drag.modifiers = MouseModifiers {
            shift: false,
            alt: true,
            ctrl: true,
        };
        // 0 (left) + 8 (alt) + 16 (ctrl) + 32 (motion).
        assert_eq!(encode(&drag, &modes).unwrap(), b"\x1b[<56;1;1M");
    }

    #[test]
    fn a_drag_is_reported_only_in_the_modes_that_asked_for_movement() {
        // `?1000` alone is presses and releases: a program in it that was sent movement would read
        // each one as another click.
        let drag = event(MouseButton::Left, MouseAction::Drag, 1, 1);
        assert_eq!(encode(&drag, &sgr()), None);
        let motion = MouseModes {
            motion: true,
            sgr: true,
            ..MouseModes::default()
        };
        assert!(encode(&drag, &motion).is_some());
    }

    #[test]
    fn the_wheel_is_buttons_64_and_65_and_never_released() {
        let modes = sgr();
        let up = event(MouseButton::WheelUp, MouseAction::Press, 2, 3);
        assert_eq!(encode(&up, &modes).unwrap(), b"\x1b[<64;4;3M");
        let down = event(MouseButton::WheelDown, MouseAction::Press, 2, 3);
        assert_eq!(encode(&down, &modes).unwrap(), b"\x1b[<65;4;3M");
        let release = event(MouseButton::WheelUp, MouseAction::Release, 2, 3);
        assert_eq!(encode(&release, &modes), None);
    }

    #[test]
    fn the_normal_encoding_offsets_by_32_and_releases_as_button_3() {
        let modes = MouseModes {
            click: true,
            ..MouseModes::default()
        };
        let press = event(MouseButton::Left, MouseAction::Press, 4, 9);
        assert_eq!(encode(&press, &modes).unwrap(), b"\x1b[M\x20\x2a\x25");
        let release = event(MouseButton::Left, MouseAction::Release, 4, 9);
        assert_eq!(encode(&release, &modes).unwrap(), b"\x1b[M\x23\x2a\x25");
    }

    #[test]
    fn a_cell_the_normal_encoding_cannot_name_is_dropped_not_misreported() {
        // Column 224 is byte 256, which wraps to a column near the left edge if pushed anyway: the
        // click would land somewhere the user never pointed.
        let modes = MouseModes {
            click: true,
            ..MouseModes::default()
        };
        let far = event(MouseButton::Left, MouseAction::Press, 0, 223);
        assert_eq!(encode(&far, &modes), None);
        let utf8 = MouseModes {
            utf8: true,
            ..modes
        };
        let bytes = encode(&far, &utf8).expect("UTF-8 reaches further");
        assert_eq!(&bytes[4..], "\u{100}!".as_bytes());
    }

    #[test]
    fn the_alternate_scroll_is_arrow_keys_in_the_programs_spelling() {
        assert_eq!(alternate_scroll(2, false), b"\x1b[A\x1b[A");
        assert_eq!(alternate_scroll(-1, true), b"\x1bOB");
        assert_eq!(alternate_scroll(1000, false).len(), 64 * 3, "capped");
    }

    #[test]
    fn the_modes_are_read_from_alacritty() {
        let modes = MouseModes::from_mode(
            TermMode::MOUSE_DRAG | TermMode::SGR_MOUSE | TermMode::ALT_SCREEN,
        );
        assert!(modes.drag && modes.sgr && modes.alt_screen && modes.reporting());
        assert!(!modes.click && !modes.utf8);
    }
}
