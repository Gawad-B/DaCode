//! Looking at files that are not text: images, PDFs, and the words inside office documents.
//!
//! A viewed file is an ordinary buffer — a tab, a place in the session, a path — whose
//! [`crate::chrome::EditorState::preview`] says what it is. That travels with its buffer the way
//! `review` does, and `EditorState::mutate` refuses an edit to it the same way, so there is no
//! second kind of tab to keep in step with the first.
//!
//! **Nothing here decodes on the UI thread.** An image is decoded, scaled to fit a texture and given
//! its mips by a worker; a PDF lives on a worker of its own for as long as its tab is open, and
//! renders a page when asked for one at a width. Each answer names the page and width it was asked
//! for, and the viewer keeps the newest it wants, so a page rendered for a zoom that has since
//! changed is simply replaced — the same version gate syntax and git answer through, keyed by what
//! the answer is *of* rather than by a counter.
//!
//! Office documents are not rendered. Their text and tables are read out into a read-only buffer,
//! which is what someone opening a `.docx` beside their code usually wants to read, and the tab says
//! that the layout is not shown.

use dacode_render::{MAX_SIDE, Picture};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc;

type Waker = crate::wake::Waker;

/// The largest file the viewer reads. Larger than the text limit, because a scanned PDF is large
/// without being pathological.
pub const MAX_BYTES: u64 = 512 * 1024 * 1024;

/// How wide a PDF page is ever rasterised, in pixels. A page is rendered at the width it is shown
/// at; past this, zooming in magnifies the raster rather than asking for a larger one.
pub const MAX_PAGE_PIXELS: u32 = 4096;

/// Points between two pages in the PDF view.
pub const PAGE_GAP: f32 = 16.0;

/// The zoom range, as a multiple of the size that fits.
pub const MIN_ZOOM: f32 = 0.1;
pub const MAX_ZOOM: f32 = 16.0;

/// What kind of not-text a file is, judged by its name.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Kind {
    Image,
    Svg,
    Pdf,
    /// A zip of XML whose text is read out: Word, Excel, PowerPoint, and their OpenDocument cousins.
    Office,
}

impl Kind {
    pub fn of(path: &Path) -> Option<Kind> {
        let extension = path.extension()?.to_str()?.to_ascii_lowercase();
        Some(match extension.as_str() {
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "ico" | "tif" | "tiff" => Kind::Image,
            "svg" => Kind::Svg,
            "pdf" => Kind::Pdf,
            "docx" | "xlsx" | "pptx" | "odt" | "ods" | "odp" => Kind::Office,
            _ => return None,
        })
    }

    /// The kind a file *opens* as. Not an SVG's: it is text, people edit it, and a code editor that
    /// would only show it as a picture is in the way. `preview_file` shows it as one on request.
    pub fn opened(path: &Path) -> Option<Kind> {
        Kind::of(path).filter(|kind| *kind != Kind::Svg)
    }

    /// Whether this kind is drawn as a picture rather than as the text of a read-only buffer.
    pub fn is_drawn(self) -> bool {
        self != Kind::Office
    }
}

/// One page: its size in points (an image's in pixels) and the best raster of it so far.
#[derive(Debug, Clone)]
pub struct Page {
    pub width: f32,
    pub height: f32,
    pub picture: Option<Arc<Picture>>,
    /// The width the last request for this page asked for, so it is asked once and not per frame.
    pub asked: Option<u32>,
}

impl Page {
    pub fn sized(width: f32, height: f32) -> Page {
        Page {
            width,
            height,
            picture: None,
            asked: None,
        }
    }
}

/// Whether the file has been read yet, and if it could not be, why.
#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    Loading,
    Ready,
    Failed(String),
}

/// What a viewed buffer shows and where in it the view is.
pub struct Preview {
    pub kind: Kind,
    pub path: PathBuf,
    pub status: Status,
    pub pages: Vec<Page>,
    /// A multiple of the size at which the widest page fits the view's width (for a PDF) or the
    /// whole picture fits the view (for an image). 1.0 is "fit".
    pub zoom: f32,
    /// How far down the pages the view is, in points — independent of the window, so a resize
    /// keeps the same place on the page.
    pub scroll: f32,
    /// Sideways, in the same units, for a page wider than the view.
    pub pan: f32,
    worker: Option<Worker>,
}

impl std::fmt::Debug for Preview {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Preview")
            .field("kind", &self.kind)
            .field("path", &self.path)
            .field("status", &self.status)
            .field("pages", &self.pages.len())
            .field("zoom", &self.zoom)
            .finish()
    }
}

/// Where a page is drawn, in pixels. The one layout drawing and the render requests both read, so a
/// page is asked for at exactly the width it is shown at.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placed {
    pub index: usize,
    pub rect: dacode_render::Rect,
}

impl Preview {
    /// Starts reading `path` on a worker. `waker` is called from it whenever it has an answer.
    pub fn open(kind: Kind, path: PathBuf, waker: Option<Waker>) -> Preview {
        let worker = kind
            .is_drawn()
            .then(|| Worker::start(kind, path.clone(), waker));
        Preview {
            kind,
            path,
            status: if kind.is_drawn() {
                Status::Loading
            } else {
                Status::Ready
            },
            pages: Vec::new(),
            zoom: 1.0,
            scroll: 0.0,
            pan: 0.0,
            worker,
        }
    }

    /// A preview with its pages already known, for tests and captures that cannot wait on a worker.
    pub fn ready(kind: Kind, path: PathBuf, pages: Vec<Page>) -> Preview {
        Preview {
            kind,
            path,
            status: Status::Ready,
            pages,
            zoom: 1.0,
            scroll: 0.0,
            pan: 0.0,
            worker: None,
        }
    }

    /// Whether the worker may still answer, so the loop knows there is something to wait for.
    pub fn busy(&self) -> bool {
        self.worker.is_some()
            && (self.status == Status::Loading
                || self.pages.iter().any(|page| {
                    page.asked
                        .is_some_and(|asked| page.width_shown() != Some(asked))
                }))
    }

    /// Takes whatever the worker has finished. Never blocks. Returns whether anything changed.
    pub fn poll(&mut self) -> bool {
        let Some(worker) = &self.worker else {
            return false;
        };
        let mut changed = false;
        while let Ok(answer) = worker.answers.try_recv() {
            changed = true;
            match answer {
                Answer::Opened(sizes) => {
                    self.pages = sizes
                        .into_iter()
                        .map(|(width, height)| Page::sized(width, height))
                        .collect();
                    self.status = if self.pages.is_empty() {
                        Status::Failed("this PDF has no pages".into())
                    } else {
                        Status::Ready
                    };
                }
                Answer::Image(picture) => {
                    self.pages = vec![Page {
                        width: picture.width as f32,
                        height: picture.height as f32,
                        picture: Some(Arc::new(picture)),
                        asked: None,
                    }];
                    self.status = Status::Ready;
                }
                Answer::Page {
                    index,
                    width,
                    picture,
                } => {
                    // Only the newest asked-for width is kept. An older one is a page rendered for
                    // a zoom that has since changed, and would replace a sharper raster with a
                    // blurrier one for a frame.
                    if let Some(page) = self.pages.get_mut(index) {
                        if page.asked == Some(width) {
                            page.picture = Some(Arc::new(picture));
                        }
                    }
                }
                Answer::Failed(why) => self.status = Status::Failed(why),
            }
        }
        changed
    }

    /// The pixels-per-point at which the picture fits `area`: the image inside it, or for a PDF
    /// the widest page across it.
    fn fit(&self, area: dacode_render::Rect, margin: f32) -> f32 {
        let width = (area.width - margin * 2.0).max(1.0);
        let height = (area.height - margin * 2.0).max(1.0);
        match self.kind {
            Kind::Pdf => {
                let widest = self.pages.iter().map(|page| page.width).fold(1.0, f32::max);
                width / widest
            }
            _ => self.pages.first().map_or(1.0, |page| {
                // An image smaller than the view is shown at its own size, not blown up to fill it:
                // a 16-pixel icon stretched across the window is not what anyone opened it to see.
                (width / page.width).min(height / page.height).min(1.0)
            }),
        }
    }

    /// Pixels per point at the current zoom.
    pub fn scale(&self, area: dacode_render::Rect, margin: f32) -> f32 {
        self.fit(area, margin) * self.zoom
    }

    /// The pages that are on screen in `area`, and where. Bounded by the view: a thousand-page PDF
    /// places the two or three pages it shows, found by adding up page heights — a sum, not a
    /// raster — and stopping past the bottom.
    pub fn placed(&self, area: dacode_render::Rect, margin: f32) -> Vec<Placed> {
        let scale = self.scale(area, margin);
        let mut out = Vec::new();
        let mut top = area.y + margin - self.scroll * scale;
        for (index, page) in self.pages.iter().enumerate() {
            let width = page.width * scale;
            let height = page.height * scale;
            let left = if width <= area.width - margin * 2.0 {
                area.x + (area.width - width) / 2.0
            } else {
                area.x + margin - self.pan * scale
            };
            let top_here = if self.kind == Kind::Pdf {
                top
            } else {
                // An image is centred vertically too, until it is taller than the view.
                (area.y + (area.height - height) / 2.0).max(top)
            };
            if top_here > area.bottom() {
                break;
            }
            if top_here + height >= area.y {
                out.push(Placed {
                    index,
                    rect: dacode_render::Rect::new(left, top_here, width, height),
                });
            }
            top += height + PAGE_GAP * scale;
        }
        out
    }

    /// How tall all the pages are together, in points — where scrolling stops.
    pub fn content_height(&self) -> f32 {
        let pages: f32 = self.pages.iter().map(|page| page.height).sum();
        pages + PAGE_GAP * self.pages.len().saturating_sub(1) as f32
    }

    /// How wide the widest page is, in points.
    fn content_width(&self) -> f32 {
        self.pages.iter().map(|page| page.width).fold(0.0, f32::max)
    }

    /// Scrolls by `pixels` down and `sideways` right at the current scale, clamped to the pages.
    pub fn scroll_by(
        &mut self,
        pixels: f32,
        sideways: f32,
        area: dacode_render::Rect,
        margin: f32,
    ) {
        let scale = self.scale(area, margin).max(f32::EPSILON);
        let tall = (area.height - margin * 2.0) / scale;
        let wide = (area.width - margin * 2.0) / scale;
        let most = (self.content_height() - tall).max(0.0);
        self.scroll = (self.scroll + pixels / scale).clamp(0.0, most);
        let across = (self.content_width() - wide).max(0.0);
        self.pan = (self.pan + sideways / scale).clamp(0.0, across);
    }

    /// Multiplies the zoom by `factor`, clamped. Back at or below the fit there is nothing to pan.
    pub fn zoom_by(&mut self, factor: f32) {
        self.zoom = (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        if self.zoom <= 1.0 {
            self.pan = 0.0;
        }
    }

    /// Back to fitting the view.
    pub fn fit_view(&mut self) {
        self.zoom = 1.0;
        self.pan = 0.0;
    }

    /// The page the top of the view is on, 0-based: what "page 3 of 12" counts.
    pub fn current_page(&self) -> usize {
        let mut bottom = 0.0;
        for (index, page) in self.pages.iter().enumerate() {
            bottom += page.height + PAGE_GAP;
            if self.scroll < bottom {
                return index;
            }
        }
        self.pages.len().saturating_sub(1)
    }

    /// Asks the worker for every placed PDF page whose raster is missing or was made for another
    /// width. Called with the frame's own layout, so only pages on screen are ever rendered.
    pub fn request(&mut self, placed: &[Placed]) {
        if self.kind != Kind::Pdf {
            return;
        }
        let Some(worker) = &self.worker else {
            return;
        };
        for at in placed {
            let width = (at.rect.width.round() as u32).clamp(1, MAX_PAGE_PIXELS);
            let Some(page) = self.pages.get_mut(at.index) else {
                continue;
            };
            if page.asked == Some(width) || page.width_shown() == Some(width) {
                continue;
            }
            page.asked = Some(width);
            let _ = worker.requests.send(Request {
                index: at.index,
                width,
            });
        }
    }

    /// What is being shown, at what size: the statusline's words for a viewed file.
    pub fn summary(&self, area: dacode_render::Rect, margin: f32) -> String {
        let percent = (self.scale(area, margin) * 100.0).round();
        match (&self.status, self.kind) {
            (Status::Loading, _) => "opening…".to_string(),
            (Status::Failed(why), _) => why.clone(),
            (_, Kind::Pdf) => format!(
                "page {} of {} · {percent}%",
                self.current_page() + 1,
                self.pages.len()
            ),
            (_, _) => match self.pages.first() {
                Some(page) => format!("{} × {} · {percent}%", page.width, page.height),
                None => String::new(),
            },
        }
    }
}

impl Page {
    fn width_shown(&self) -> Option<u32> {
        self.picture.as_ref().map(|picture| picture.width)
    }
}

struct Request {
    index: usize,
    width: u32,
}

enum Answer {
    /// A PDF was read: each page's size in points.
    Opened(Vec<(f32, f32)>),
    Image(Picture),
    Page {
        index: usize,
        width: u32,
        picture: Picture,
    },
    Failed(String),
}

/// The thread that reads one file and, for a PDF, renders its pages for as long as the tab is open.
/// Dropping it closes the request channel, which ends the thread.
struct Worker {
    requests: mpsc::Sender<Request>,
    answers: mpsc::Receiver<Answer>,
}

impl Worker {
    fn start(kind: Kind, path: PathBuf, waker: Option<Waker>) -> Worker {
        let (requests, incoming) = mpsc::channel::<Request>();
        let (outgoing, answers) = mpsc::channel::<Answer>();
        let send = move |answer: Answer| {
            let sent = outgoing.send(answer).is_ok();
            if let Some(wake) = &waker {
                wake();
            }
            sent
        };
        std::thread::Builder::new()
            .name("dacode viewer".into())
            .spawn(move || {
                let bytes = match read_bounded(&path) {
                    Ok(bytes) => bytes,
                    Err(why) => {
                        send(Answer::Failed(why));
                        return;
                    }
                };
                match kind {
                    Kind::Image => {
                        send(match decode_image(&bytes) {
                            Ok(picture) => Answer::Image(picture),
                            Err(why) => Answer::Failed(why),
                        });
                    }
                    Kind::Svg => {
                        send(match decode_svg(&bytes) {
                            Ok(picture) => Answer::Image(picture),
                            Err(why) => Answer::Failed(why),
                        });
                    }
                    Kind::Pdf => serve_pdf(bytes, &incoming, &send),
                    Kind::Office => {}
                }
            })
            .expect("a thread can be started");
        Worker { requests, answers }
    }
}

fn read_bounded(path: &Path) -> Result<Vec<u8>, String> {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    match std::fs::metadata(path) {
        Ok(data) if data.len() > MAX_BYTES => {
            return Err(format!(
                "{name} is {} MB, larger than the {} MB the viewer opens",
                data.len() / (1024 * 1024),
                MAX_BYTES / (1024 * 1024)
            ));
        }
        Err(err) => return Err(format!("could not open {name}: {err}")),
        Ok(_) => {}
    }
    std::fs::read(path).map_err(|err| format!("could not read {name}: {err}"))
}

/// An image file as a picture no larger than a texture may be.
pub fn decode_image(bytes: &[u8]) -> Result<Picture, String> {
    let image = image::load_from_memory(bytes)
        .map_err(|err| format!("this image could not be decoded: {err}"))?;
    let image = if image.width() > MAX_SIDE || image.height() > MAX_SIDE {
        image.resize(MAX_SIDE, MAX_SIDE, image::imageops::FilterType::Triangle)
    } else {
        image
    };
    let rgba = image.to_rgba8();
    let (width, height) = rgba.dimensions();
    if width == 0 || height == 0 {
        return Err("this image has no pixels".into());
    }
    Ok(Picture::new(width, height, rgba.into_raw()))
}

/// An SVG drawn at its own size, or larger when that is small: an icon's 24 pixels would be a blur
/// at any zoom, so it is rasterised at least 1024 pixels across.
pub fn decode_svg(bytes: &[u8]) -> Result<Picture, String> {
    let tree = resvg::usvg::Tree::from_data(bytes, &resvg::usvg::Options::default())
        .map_err(|err| format!("this SVG could not be read: {err}"))?;
    let size = tree.size();
    let longest = size.width().max(size.height()).max(1.0);
    let scale = (1024.0 / longest).max(1.0).min(MAX_SIDE as f32 / longest);
    let width = ((size.width() * scale).ceil() as u32).clamp(1, MAX_SIDE);
    let height = ((size.height() * scale).ceil() as u32).clamp(1, MAX_SIDE);
    let mut pixmap = resvg::tiny_skia::Pixmap::new(width, height)
        .ok_or_else(|| "this SVG has no size to draw".to_string())?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    Ok(Picture::new(width, height, unpremultiply(pixmap.take())))
}

/// tiny-skia and vello hand back premultiplied pixels; the texture wants straight alpha.
fn unpremultiply(mut rgba: Vec<u8>) -> Vec<u8> {
    for pixel in rgba.chunks_exact_mut(4) {
        let alpha = pixel[3] as u32;
        if alpha != 0 && alpha != 255 {
            for channel in &mut pixel[..3] {
                *channel = ((*channel as u32 * 255 + alpha / 2) / alpha).min(255) as u8;
            }
        }
    }
    rgba
}

/// Reads a PDF once and renders pages as they are asked for, newest request first: scrolling
/// quickly through a long document asks for many pages, and the ones it has since left are the
/// least worth drawing.
fn serve_pdf(bytes: Vec<u8>, incoming: &mpsc::Receiver<Request>, send: &dyn Fn(Answer) -> bool) {
    use hayro::hayro_syntax::Pdf;
    let pdf = match Pdf::new(bytes) {
        Ok(pdf) => pdf,
        Err(err) => {
            send(Answer::Failed(format!(
                "this PDF could not be read ({err:?}) — encrypted ones are not supported yet"
            )));
            return;
        }
    };
    let pages = pdf.pages();
    let sizes = pages.iter().map(|page| page.render_dimensions()).collect();
    if !send(Answer::Opened(sizes)) {
        return;
    }
    let cache = hayro::RenderCache::new();
    let interpreter = hayro::hayro_interpret::InterpreterSettings::default();
    let settings = hayro::RenderSettings::default();
    while let Ok(first) = incoming.recv() {
        let mut queue = vec![first];
        queue.extend(incoming.try_iter());
        // The newest request for each page is the one that counts; older widths are stale.
        let mut seen = std::collections::HashSet::new();
        for request in queue.into_iter().rev() {
            if !seen.insert(request.index) {
                continue;
            }
            let Some(page) = pages.get(request.index) else {
                continue;
            };
            let (points, _) = page.render_dimensions();
            let scale = request.width as f32 / points.max(1.0);
            let pixmap = hayro::render(
                page,
                &cache,
                &interpreter,
                &settings,
                &hayro::PixmapSettings {
                    x_scale: scale,
                    y_scale: scale,
                    bg_color: hayro::vello_cpu::color::palette::css::WHITE,
                },
            );
            let (width, height) = (pixmap.width() as u32, pixmap.height() as u32);
            if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE {
                continue;
            }
            let rgba = unpremultiply(pixmap.data_as_u8_slice().to_vec());
            let answer = Answer::Page {
                index: request.index,
                width: request.width,
                picture: Picture::new(width, height, rgba),
            };
            if !send(answer) {
                return;
            }
        }
    }
}

/// The text of an office document, its paragraphs as lines and its table cells separated by ` │ `,
/// headed by a line saying what this is. `Err` is a sentence for the status line.
pub fn office_text(path: &Path) -> Result<String, String> {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let file = std::fs::File::open(path).map_err(|err| format!("could not open {name}: {err}"))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|_| format!("{name} is not an office document DaCode can read"))?;
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let mut read = |entry: &str| -> Option<String> {
        use std::io::Read;
        let file = archive.by_name(entry).ok()?;
        // Bounded: a document's XML is a few megabytes, and a zip bomb is not one.
        let mut text = String::new();
        file.take(64 * 1024 * 1024).read_to_string(&mut text).ok()?;
        Some(text)
    };
    let body = match extension.as_str() {
        "docx" => read("word/document.xml").map(|xml| extract(&xml, &WORD)),
        "pptx" => {
            let mut slides = Vec::new();
            for number in 1..=2000 {
                match read(&format!("ppt/slides/slide{number}.xml")) {
                    Some(xml) => slides.push(format!(
                        "── slide {number} ──\n{}",
                        extract(&xml, &SLIDES).trim()
                    )),
                    None => break,
                }
            }
            (!slides.is_empty()).then(|| slides.join("\n\n"))
        }
        "xlsx" => {
            let shared = read("xl/sharedStrings.xml")
                .map(|xml| shared_strings(&xml))
                .unwrap_or_default();
            let mut sheets = Vec::new();
            for number in 1..=500 {
                match read(&format!("xl/worksheets/sheet{number}.xml")) {
                    Some(xml) => sheets.push(format!(
                        "── sheet {number} ──\n{}",
                        sheet_text(&xml, &shared).trim_end()
                    )),
                    None => break,
                }
            }
            (!sheets.is_empty()).then(|| sheets.join("\n\n"))
        }
        "odt" | "ods" | "odp" => read("content.xml").map(|xml| extract(&xml, &OPEN_DOCUMENT)),
        _ => None,
    }
    .ok_or_else(|| format!("{name} has no text DaCode can find"))?;
    Ok(format!(
        "{name} — the text of this document, read only. Its layout, pictures and styles are not \
         shown.\n\n{}\n",
        body.trim()
    ))
}

/// Which tags mean what, for one family of formats.
struct Vocabulary {
    /// Elements whose text is the document's text.
    text: &'static [&'static str],
    /// Closing these ends a line.
    line: &'static [&'static str],
    /// Closing these ends a table cell.
    cell: &'static [&'static str],
    /// Empty elements that stand for a tab.
    tab: &'static [&'static str],
}

const WORD: Vocabulary = Vocabulary {
    text: &["w:t"],
    line: &["w:p", "w:tr"],
    cell: &["w:tc"],
    tab: &["w:tab"],
};

const SLIDES: Vocabulary = Vocabulary {
    text: &["a:t"],
    line: &["a:p"],
    cell: &["a:tc"],
    tab: &[],
};

const OPEN_DOCUMENT: Vocabulary = Vocabulary {
    text: &["text:p", "text:h", "text:span", "text:a"],
    line: &["text:p", "text:h", "table:table-row"],
    cell: &["table:table-cell"],
    tab: &["text:tab"],
};

/// A tag as the scanner meets it.
enum Tag<'a> {
    Open(&'a str),
    Close(&'a str),
    Empty(&'a str),
}

/// Splits XML into text and tags, by hand: these files are machine-written and regular, and the
/// scanner needs only element names and character data, so a full parser would be a dependency
/// spent on attributes and namespaces nothing here reads.
fn scan<'a>(xml: &'a str, mut visit: impl FnMut(Option<Tag<'a>>, &'a str)) {
    let name = |text: &'a str| {
        text.split(|c: char| c.is_whitespace() || c == '/')
            .next()
            .unwrap_or("")
    };
    let mut rest = xml;
    while let Some(open) = rest.find('<') {
        if open > 0 {
            visit(None, &rest[..open]);
        }
        let Some(close) = rest[open..].find('>') else {
            return;
        };
        let inner = &rest[open + 1..open + close];
        rest = &rest[open + close + 1..];
        if inner.starts_with('?') || inner.starts_with('!') {
            continue;
        }
        let tag = if let Some(closing) = inner.strip_prefix('/') {
            Tag::Close(name(closing))
        } else if inner.ends_with('/') {
            Tag::Empty(name(inner))
        } else {
            Tag::Open(name(inner))
        };
        visit(Some(tag), "");
    }
    if !rest.is_empty() {
        visit(None, rest);
    }
}

fn extract(xml: &str, words: &Vocabulary) -> String {
    let mut out = String::new();
    let mut depth = 0usize;
    scan(xml, |tag, text| match tag {
        None if depth > 0 => out.push_str(&unescape(text)),
        None => {}
        Some(Tag::Open(name)) if words.text.contains(&name) => depth += 1,
        Some(Tag::Close(name)) => {
            if words.text.contains(&name) {
                depth = depth.saturating_sub(1);
            }
            if words.cell.contains(&name) {
                out.push_str(" │ ");
            }
            if words.line.contains(&name) {
                let kept = out.trim_end_matches(" │ ").len();
                out.truncate(kept);
                out.push('\n');
            }
        }
        Some(Tag::Empty(name)) if words.tab.contains(&name) => out.push('\t'),
        Some(_) => {}
    });
    // Runs of blank lines, which empty paragraphs leave, down to one.
    let mut tidy = String::with_capacity(out.len());
    let mut blank = 0;
    for line in out.lines() {
        if line.trim().is_empty() {
            blank += 1;
            if blank > 1 {
                continue;
            }
        } else {
            blank = 0;
        }
        tidy.push_str(line.trim_end());
        tidy.push('\n');
    }
    tidy
}

/// Excel keeps each distinct string once, in `sharedStrings.xml`, and cells point at them by index.
fn shared_strings(xml: &str) -> Vec<String> {
    let mut strings = Vec::new();
    let mut current: Option<String> = None;
    let mut in_text = false;
    scan(xml, |tag, text| match tag {
        Some(Tag::Open("si")) => current = Some(String::new()),
        Some(Tag::Close("si")) => strings.extend(current.take()),
        Some(Tag::Open("t")) => in_text = true,
        Some(Tag::Close("t")) => in_text = false,
        None if in_text => {
            if let Some(current) = &mut current {
                current.push_str(&unescape(text));
            }
        }
        _ => {}
    });
    strings
}

/// A worksheet's rows, cells separated by ` │ `. A cell of type `s` is an index into the shared
/// strings; anything else is its value as written, which for a formula is the last result Excel
/// saved.
fn sheet_text(xml: &str, shared: &[String]) -> String {
    let mut out = String::new();
    let mut row: Vec<String> = Vec::new();
    let mut shared_cell = false;
    let mut in_value = false;
    let mut rest = xml;
    // Attributes matter here (`t="s"`), so cells are found by their opening tag's text.
    while let Some(at) = rest.find('<') {
        if in_value {
            let value = unescape(&rest[..at]);
            let shown = if shared_cell {
                value
                    .trim()
                    .parse::<usize>()
                    .ok()
                    .and_then(|index| shared.get(index).cloned())
                    .unwrap_or(value)
            } else {
                value
            };
            if let Some(last) = row.last_mut() {
                last.push_str(&shown);
            }
        }
        let Some(end) = rest[at..].find('>') else {
            break;
        };
        let tag = &rest[at + 1..at + end];
        rest = &rest[at + end + 1..];
        if tag == "c" || tag.starts_with("c ") {
            shared_cell = tag.contains("t=\"s\"") && !tag.ends_with('/');
            row.push(String::new());
        } else if tag == "v" || tag == "t" || tag.starts_with("t ") {
            in_value = true;
        } else if tag == "/v" || tag == "/t" {
            in_value = false;
        } else if tag == "/row" {
            while row.last().is_some_and(|cell| cell.is_empty()) {
                row.pop();
            }
            out.push_str(&row.join(" │ "));
            out.push('\n');
            row.clear();
        }
    }
    out
}

fn unescape(text: &str) -> String {
    if !text.contains('&') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        let Some(end) = rest[at..].find(';') else {
            out.push_str(&rest[at..]);
            return out;
        };
        let entity = &rest[at + 1..at + end];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => entity
                .strip_prefix("#x")
                .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                .or_else(|| entity.strip_prefix('#').and_then(|dec| dec.parse().ok()))
                .and_then(char::from_u32),
        };
        match decoded {
            Some(c) => out.push(c),
            None => out.push_str(&rest[at..at + end + 1]),
        }
        rest = &rest[at + end + 1..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use dacode_render::Rect;
    use std::io::Write;

    fn scratch(label: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("dacode-viewer-{}-{label}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn zip(path: &Path, entries: &[(&str, &str)]) {
        let mut writer = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        for (name, body) in entries {
            writer
                .start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(body.as_bytes()).unwrap();
        }
        writer.finish().unwrap();
    }

    #[test]
    fn a_files_name_decides_how_it_is_viewed() {
        assert_eq!(Kind::of(Path::new("a/photo.JPG")), Some(Kind::Image));
        assert_eq!(Kind::of(Path::new("logo.svg")), Some(Kind::Svg));
        assert_eq!(Kind::of(Path::new("paper.pdf")), Some(Kind::Pdf));
        assert_eq!(Kind::of(Path::new("notes.docx")), Some(Kind::Office));
        assert_eq!(Kind::of(Path::new("main.rs")), None);
        assert!(
            !Kind::Office.is_drawn(),
            "office text is a buffer, not a picture"
        );
    }

    #[test]
    fn a_word_documents_paragraphs_and_table_cells_come_out_as_lines() {
        let dir = scratch("docx");
        let path = dir.join("report.docx");
        zip(
            &path,
            &[(
                "word/document.xml",
                r#"<?xml version="1.0"?><w:document><w:body>
                <w:p><w:r><w:t>Quarterly</w:t></w:r><w:r><w:t xml:space="preserve"> report &amp; notes</w:t></w:r></w:p>
                <w:p/>
                <w:tbl><w:tr><w:tc><w:p><w:r><w:t>Name</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>Score</w:t></w:r></w:p></w:tc></w:tr></w:tbl>
                </w:body></w:document>"#,
            )],
        );
        let text = office_text(&path).unwrap();
        assert!(text.contains("Quarterly report & notes"), "{text}");
        assert!(text.contains("Name") && text.contains("Score"), "{text}");
        assert!(
            text.lines().next().unwrap().contains("read only"),
            "the first line says what this is"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_spreadsheets_shared_strings_are_looked_up_into_its_rows() {
        // Excel stores each distinct string once and points cells at it: reading the cell's value
        // as written gives "0", "1", "2" instead of the words.
        let dir = scratch("xlsx");
        let path = dir.join("scores.xlsx");
        zip(
            &path,
            &[
                (
                    "xl/sharedStrings.xml",
                    "<sst><si><t>name</t></si><si><t>score</t></si><si><t>Ada</t></si></sst>",
                ),
                (
                    "xl/worksheets/sheet1.xml",
                    r#"<worksheet><sheetData>
                    <row r="1"><c r="A1" t="s"><v>0</v></c><c r="B1" t="s"><v>1</v></c></row>
                    <row r="2"><c r="A2" t="s"><v>2</v></c><c r="B2"><v>98.5</v></c></row>
                    </sheetData></worksheet>"#,
                ),
            ],
        );
        let text = office_text(&path).unwrap();
        assert!(text.contains("name │ score"), "{text}");
        assert!(text.contains("Ada │ 98.5"), "{text}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_presentation_is_read_slide_by_slide_in_order() {
        let dir = scratch("pptx");
        let path = dir.join("talk.pptx");
        zip(
            &path,
            &[
                (
                    "ppt/slides/slide1.xml",
                    "<p:sld><a:p><a:r><a:t>Hello</a:t></a:r></a:p></p:sld>",
                ),
                (
                    "ppt/slides/slide2.xml",
                    "<p:sld><a:p><a:r><a:t>Thanks</a:t></a:r></a:p></p:sld>",
                ),
            ],
        );
        let text = office_text(&path).unwrap();
        assert!(text.contains("slide 1") && text.contains("slide 2"));
        assert!(text.find("Hello").unwrap() < text.find("Thanks").unwrap());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_file_that_is_not_a_zip_is_refused_in_a_sentence() {
        let dir = scratch("not-zip");
        let path = dir.join("fake.docx");
        std::fs::write(&path, "plain text").unwrap();
        let why = office_text(&path).unwrap_err();
        assert!(why.contains("fake.docx"), "{why}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn entities_are_decoded_and_an_unknown_one_is_left_as_written() {
        assert_eq!(
            unescape("a &lt;b&gt; &#233; &#x263A; &nope;"),
            "a <b> é ☺ &nope;"
        );
    }

    #[test]
    fn a_png_decodes_to_its_own_size_and_a_corrupt_one_says_so() {
        let mut bytes = Vec::new();
        image::RgbaImage::from_pixel(3, 2, image::Rgba([10, 20, 30, 255]))
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        let picture = decode_image(&bytes).unwrap();
        assert_eq!((picture.width, picture.height), (3, 2));
        assert!(decode_image(b"not an image").is_err());
    }

    #[test]
    fn an_svg_icon_is_rasterised_large_enough_to_zoom_into() {
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="12"><rect width="24" height="12" fill="red"/></svg>"#;
        let picture = decode_svg(svg).unwrap();
        assert_eq!((picture.width, picture.height), (1024, 512));
    }

    fn pages(count: usize) -> Preview {
        Preview::ready(
            Kind::Pdf,
            PathBuf::from("a.pdf"),
            (0..count).map(|_| Page::sized(600.0, 800.0)).collect(),
        )
    }

    #[test]
    fn only_the_pages_on_screen_are_placed_however_long_the_document() {
        // The viewer's version of the one rule: a thousand-page PDF costs what its visible pages
        // cost, because only those are placed and only those are ever asked to render.
        let area = Rect::new(0.0, 0.0, 600.0, 900.0);
        let short = pages(3).placed(area, 0.0);
        let long = pages(1000).placed(area, 0.0);
        assert_eq!(short.len(), long.len());
        assert!(long.len() <= 2, "{long:?}");
        assert_eq!(long[0].rect.width, 600.0, "fitted to the width");
    }

    #[test]
    fn scrolling_stops_at_the_last_page_and_counts_pages_as_it_goes() {
        let area = Rect::new(0.0, 0.0, 600.0, 800.0);
        let mut preview = pages(3);
        preview.scroll_by(1_000_000.0, 0.0, area, 0.0);
        assert_eq!(preview.scroll, preview.content_height() - 800.0);
        assert_eq!(preview.current_page(), 2);
        preview.scroll_by(-1_000_000.0, 0.0, area, 0.0);
        assert_eq!(preview.scroll, 0.0);
        assert_eq!(preview.current_page(), 0);
    }

    #[test]
    fn a_small_image_is_shown_at_its_own_size_not_stretched_to_the_window() {
        let preview = Preview::ready(
            Kind::Image,
            PathBuf::from("icon.png"),
            vec![Page::sized(16.0, 16.0)],
        );
        let placed = preview.placed(Rect::new(0.0, 0.0, 800.0, 600.0), 0.0);
        assert_eq!(placed[0].rect.width, 16.0);
        assert_eq!(placed[0].rect.x, 392.0, "centred");
    }

    /// A two-page PDF with a correct cross-reference table, built here so the test needs no file.
    fn two_page_pdf() -> Vec<u8> {
        let stream = "0.2 0.4 0.8 rg 72 72 200 100 re f";
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 5 0 R >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Contents 5 0 R >>".to_string(),
            format!(
                "<< /Length {} >>\nstream\n{stream}\nendstream",
                stream.len()
            ),
        ];
        let mut out = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (index, object) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.extend(format!("{} 0 obj\n{object}\nendobj\n", index + 1).into_bytes());
        }
        let xref = out.len();
        out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).into_bytes());
        for offset in offsets {
            out.extend(format!("{offset:010} 00000 n \n").into_bytes());
        }
        out.extend(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
                objects.len() + 1
            )
            .into_bytes(),
        );
        out
    }

    #[test]
    fn a_pdf_is_read_off_the_ui_thread_and_renders_a_page_at_the_width_asked() {
        let dir = scratch("pdf");
        let path = dir.join("paper.pdf");
        std::fs::write(&path, two_page_pdf()).unwrap();
        let mut preview = Preview::open(Kind::Pdf, path, None);
        let wait = |preview: &mut Preview, done: &dyn Fn(&Preview) -> bool| {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
            while !done(preview) && std::time::Instant::now() < deadline {
                preview.poll();
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        };
        wait(&mut preview, &|p| p.status != Status::Loading);
        assert_eq!(preview.status, Status::Ready);
        assert_eq!(preview.pages.len(), 2);
        assert_eq!(
            (preview.pages[1].width, preview.pages[1].height),
            (300.0, 400.0)
        );

        let area = Rect::new(0.0, 0.0, 612.0, 900.0);
        let placed = preview.placed(area, 0.0);
        preview.request(&placed);
        wait(&mut preview, &|p| p.pages[0].picture.is_some());
        let picture = preview.pages[0]
            .picture
            .as_ref()
            .expect("page one rendered");
        assert_eq!(picture.width, 612, "at the width it is shown at");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_file_that_is_not_a_pdf_fails_in_a_sentence_rather_than_hanging() {
        let dir = scratch("bad-pdf");
        let path = dir.join("broken.pdf");
        std::fs::write(&path, "this is not a pdf").unwrap();
        let mut preview = Preview::open(Kind::Pdf, path, None);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while preview.status == Status::Loading && std::time::Instant::now() < deadline {
            preview.poll();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(
            matches!(preview.status, Status::Failed(_)),
            "{:?}",
            preview.status
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_stale_page_raster_is_not_shown_over_a_newer_request() {
        let mut preview = pages(1);
        preview.pages[0].asked = Some(800);
        let (requests, _incoming) = mpsc::channel();
        let (send, answers) = mpsc::channel();
        preview.worker = Some(Worker { requests, answers });
        let tiny = || Picture::new(1, 1, vec![0; 4]);
        send.send(Answer::Page {
            index: 0,
            width: 400,
            picture: tiny(),
        })
        .unwrap();
        preview.poll();
        assert!(
            preview.pages[0].picture.is_none(),
            "rendered for a zoom since left"
        );
        send.send(Answer::Page {
            index: 0,
            width: 800,
            picture: tiny(),
        })
        .unwrap();
        preview.poll();
        assert!(preview.pages[0].picture.is_some());
    }
}
