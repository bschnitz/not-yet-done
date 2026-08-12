//! Inline terminal images for markdown bodies (kitty / sixel / iTerm2 /
//! halfblocks, whatever the terminal answered to the graphics query).
//!
//! One [`ImageStore`] per content view holds everything that is *not* pure
//! text about the pictures in that view's rows:
//!
//! * the terminal's graphics capability ([`Picker`], queried once at startup),
//! * decoded pixels per URL, plus the "still loading" / "gave up" bookkeeping,
//! * the URLs the view would like to have (`wanted`) — the App pump drains
//!   them and fetches through the view's adapter, because only the adapter
//!   knows how to authenticate against its host,
//! * the terminal-protocol objects (`SlicedProtocol`) that actually emit
//!   escape sequences.
//!
//! It plays two roles for two different crates, deliberately from one struct
//! so both see the same cache:
//!
//! * [`ImageResolver`] — asked by `ratatui-markdown` while *parsing* a body.
//!   A cache hit hands over the pixels and the renderer reserves blank lines
//!   for them; a miss queues the URL and returns `None`, which degrades to the
//!   `[image: …]` fallback span until the download lands.
//! * [`ImagePainter`] — asked by our table widget while *drawing*, once per
//!   visible picture, after all text. See [`not_yet_done_ratatui::ImageDraw`].
//!
//! Nothing here does I/O: downloading is the App's job (it owns the adapter
//! and the runtime), decoding happens off-thread, and the result comes back
//! through [`insert_decoded`](ImageStore::insert_decoded).

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::sync::OnceLock;

use image::DynamicImage;
use ratatui::buffer::Buffer;
use ratatui::layout::{Rect, Size};
use ratatui::widgets::Widget;
use ratatui_image::Resize;
use ratatui_image::picker::Picker;
use ratatui_image::sliced::{SignedPosition, SlicedImage, SlicedProtocol};
use ratatui_markdown::markdown::ImageResolver;

use not_yet_done_ratatui::{ImageDraw, ImageLineRef, ImagePainter};

/// A decoded picture, shared between the store and the download task.
pub type DecodedImage = Arc<DynamicImage>;

/// Process-wide graphics facts: what the terminal can do, and how tall the
/// user lets a picture get. Both are settled once at startup, so neither has
/// to be threaded through view construction (a [`ContentPane`] is built in a
/// dozen places, none of which sees the TUI config).
///
/// [`ContentPane`]: crate::views::content_view::ContentPane
#[derive(Debug, Clone)]
struct Graphics {
    /// `None` = no inline images: the query failed, the terminal can't do
    /// graphics, or the user switched the feature off.
    picker: Option<Picker>,
    max_height: u16,
}

static GRAPHICS: OnceLock<Graphics> = OnceLock::new();

/// Fallback for the paths that never call [`init_terminal_graphics`]: unit
/// tests and the CLI. No graphics, so the cap is only there to keep the
/// arithmetic well-defined.
const DEFAULT_MAX_HEIGHT: u16 = 20;

/// Ask the terminal which graphics protocol it speaks and remember the answer
/// together with the configured height cap.
///
/// Must be called **after** entering the alternate screen (raw mode on) and
/// **before** the event reader starts, because the query writes an escape
/// sequence to stdout and reads the reply straight off stdin — a concurrent
/// reader would swallow it. Calling it twice is a no-op.
///
/// `enabled == false` records "no graphics" without querying at all, so a
/// user who switched inline images off in `tui.yaml` doesn't pay the 2 s
/// stdin timeout of an unresponsive terminal.
pub fn init_terminal_graphics(enabled: bool, max_height: u16) {
    let _ = GRAPHICS.get_or_init(|| Graphics {
        picker: if enabled {
            Picker::from_query_stdio().ok()
        } else {
            None
        },
        max_height: max_height.max(1),
    });
}

fn graphics() -> Graphics {
    GRAPHICS.get().cloned().unwrap_or_else(|| Graphics {
        picker: None,
        max_height: DEFAULT_MAX_HEIGHT,
    })
}

/// Cap on the decoded pixel size, as a multiple of the configured maximum
/// cell height. A picture is never displayed larger than `max_height` rows,
/// so keeping more than a few times that many pixel rows around only wastes
/// memory and makes every clone (the markdown renderer takes one per render)
/// more expensive.
const DECODE_HEIGHT_SLACK: u32 = 2;

/// Cap on the decoded pixel width, in cells' worth of pixels. Wide enough for
/// any real terminal, narrow enough to bound a maliciously large screenshot.
const DECODE_MAX_CELLS_WIDE: u32 = 400;

/// How many protocol objects (encoded escape sequences / transmitted kitty
/// images) to keep before dropping the lot. Re-encoding is cheap compared to
/// the download, and a chat scrolled far enough to blow the cap has long left
/// the old pictures behind.
const PROTOCOL_CACHE_CAP: usize = 48;

/// What one [`ImageLineRef::key`] stands for: a URL rendered at a specific
/// cell size. The size is part of the identity, so a resized terminal simply
/// produces new keys instead of reusing a stale protocol.
#[derive(Debug, Clone)]
struct Slot {
    url: String,
    size: Size,
}

/// Per-view image cache, resolver and painter. See the module docs.
pub struct ImageStore {
    /// The terminal capability, bound **lazily** on first use: a pane is
    /// built while the app is still starting up, before the alternate screen
    /// exists and therefore before the terminal can be asked anything. Tests
    /// pre-fill it via [`ImageStore::with_picker`].
    picker: std::cell::OnceCell<Option<Picker>>,
    /// Maximum height of one picture in terminal rows, from `tui.yaml`.
    /// Bound lazily for the same reason.
    max_height: std::cell::OnceCell<u16>,
    decoded: HashMap<String, DecodedImage>,
    /// URLs whose download or decode failed — never retried, so a broken
    /// attachment costs one request, not one per rebuild.
    failed: HashSet<String>,
    /// URLs handed to the App pump and not yet answered.
    inflight: HashSet<String>,
    /// URLs the last render wanted but didn't have. Drained by the pump.
    wanted: Vec<String>,
    slots: HashMap<u64, Slot>,
    protocols: HashMap<u64, SlicedProtocol>,
}

impl std::fmt::Debug for ImageStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImageStore")
            .field("enabled", &self.picker().is_some())
            .field("max_height", &self.max_height())
            .field("decoded", &self.decoded.len())
            .field("failed", &self.failed.len())
            .field("inflight", &self.inflight.len())
            .field("wanted", &self.wanted.len())
            .finish()
    }
}

impl ImageStore {
    /// A store that binds to whatever [`init_terminal_graphics`] recorded,
    /// the first time it is actually used.
    pub fn new() -> Self {
        Self {
            picker: std::cell::OnceCell::new(),
            max_height: std::cell::OnceCell::new(),
            decoded: HashMap::new(),
            failed: HashSet::new(),
            inflight: HashSet::new(),
            wanted: Vec::new(),
            slots: HashMap::new(),
            protocols: HashMap::new(),
        }
    }

    /// A store with an explicit capability, bound up front — for tests,
    /// which must not depend on the terminal the suite happens to run in.
    pub fn with_picker(picker: Option<Picker>, max_height: u16) -> Self {
        let store = Self::new();
        let _ = store.picker.set(picker);
        let _ = store.max_height.set(max_height.max(1));
        store
    }

    /// A store that can never show anything — for the paths that build a
    /// pane but have no terminal (tests, headless rendering).
    pub fn disabled() -> Self {
        Self::with_picker(None, DEFAULT_MAX_HEIGHT)
    }

    /// The terminal capability, binding it on first call. Everything that
    /// needs the picker goes through here rather than the field, so a store
    /// built before the alternate screen existed still gets the right answer.
    fn picker(&self) -> Option<&Picker> {
        self.picker.get_or_init(|| graphics().picker).as_ref()
    }

    /// Whether this store can show pictures at all. When false the markdown
    /// path skips image handling entirely and every `![…]` stays a fallback
    /// span, so no URL is ever queued.
    pub fn enabled(&self) -> bool {
        self.picker().is_some()
    }

    /// Take the URLs that want downloading, marking them in-flight so a
    /// rebuild between request and answer doesn't queue them twice.
    pub fn take_wanted(&mut self) -> Vec<String> {
        let urls = std::mem::take(&mut self.wanted);
        for url in &urls {
            self.inflight.insert(url.clone());
        }
        urls
    }

    /// Whether anything is currently waiting for bytes — used to decide
    /// whether an arriving image is worth a table rebuild.
    pub fn has_inflight(&self) -> bool {
        !self.inflight.is_empty()
    }

    /// Hand in the bytes fetched for `url`.
    ///
    /// Returns `true` when the picture became displayable, i.e. when the
    /// caller should rebuild the table so the reserved lines appear. `Err`
    /// (download failed) and undecodable bytes both mark the URL failed.
    pub fn insert_decoded(&mut self, url: &str, image: Option<DecodedImage>) -> bool {
        self.inflight.remove(url);
        match image {
            Some(img) => {
                self.decoded.insert(url.to_string(), img);
                // A previously built protocol (from an earlier, smaller
                // decode) must not outlive its pixels.
                self.invalidate(url);
                true
            }
            None => {
                self.failed.insert(url.to_string());
                false
            }
        }
    }

    /// Downscale freshly downloaded bytes to the largest size that can ever
    /// be shown, then decode. Runs off the UI thread (the App calls it from a
    /// blocking task) because both steps are pure CPU.
    ///
    /// Aspect ratio is preserved, so the cell arithmetic in
    /// [`ImageResolver::cell_dimensions`] is unaffected by the shrink.
    pub fn decode_bytes(bytes: &[u8], max_height: u16, font: (u16, u16)) -> Option<DecodedImage> {
        let img = image::load_from_memory(bytes).ok()?;
        let max_w = DECODE_MAX_CELLS_WIDE * font.0.max(1) as u32;
        let max_h = DECODE_HEIGHT_SLACK * max_height.max(1) as u32 * font.1.max(1) as u32;
        let img = if img.width() > max_w || img.height() > max_h {
            img.resize(max_w, max_h, image::imageops::FilterType::Triangle)
        } else {
            img
        };
        Some(Arc::new(img))
    }

    /// Maximum height of one picture in terminal rows, as configured.
    pub fn max_height(&self) -> u16 {
        *self.max_height.get_or_init(|| graphics().max_height)
    }

    /// The terminal's cell size in pixels, or a plausible default when there
    /// is no picker (so a caller can still do the arithmetic).
    pub fn font_size(&self) -> (u16, u16) {
        match self.picker() {
            Some(p) => {
                let fs = p.font_size();
                (fs.width.max(1), fs.height.max(1))
            }
            None => (9, 18),
        }
    }

    /// Register a picture at a concrete cell size and return the handle the
    /// table lines carry. Same URL at the same size ⇒ same key, so scrolling
    /// and rebuilding reuse the encoded protocol.
    pub fn register(&mut self, url: &str, width: u16, height: u16) -> u64 {
        let mut hasher = DefaultHasher::new();
        url.hash(&mut hasher);
        width.hash(&mut hasher);
        height.hash(&mut hasher);
        let key = hasher.finish();
        self.slots.entry(key).or_insert_with(|| Slot {
            url: url.to_string(),
            size: Size::new(width, height),
        });
        key
    }

    /// Build the per-line back-references for one registered picture: `height`
    /// refs that differ only in `row_in_image`, so whichever one survives
    /// scrolling still knows where the top edge is.
    pub fn line_refs(&self, key: u64, col: u16) -> Vec<ImageLineRef> {
        let Some(slot) = self.slots.get(&key) else {
            return Vec::new();
        };
        (0..slot.size.height)
            .map(|row_in_image| ImageLineRef {
                key,
                col,
                width: slot.size.width,
                height: slot.size.height,
                row_in_image,
            })
            .collect()
    }

    /// Drop every protocol built from `url`'s pixels.
    fn invalidate(&mut self, url: &str) {
        let keys: Vec<u64> = self
            .slots
            .iter()
            .filter(|(_, s)| s.url == url)
            .map(|(k, _)| *k)
            .collect();
        for k in keys {
            self.protocols.remove(&k);
        }
    }

    /// Encode (and for kitty: transmit) the picture behind `key`, unless that
    /// already happened. `false` means it can't be drawn right now.
    fn ensure_protocol(&mut self, key: u64) -> bool {
        if self.protocols.contains_key(&key) {
            return true;
        }
        let Some(picker) = self.picker().cloned() else {
            return false;
        };
        let Some(slot) = self.slots.get(&key).cloned() else {
            return false;
        };
        let Some(img) = self.decoded.get(&slot.url).cloned() else {
            return false;
        };
        let proto = match SlicedProtocol::new_with_resize(
            &picker,
            (*img).clone(),
            slot.size,
            Resize::Fit(None),
        ) {
            Ok(p) => p,
            Err(_) => {
                // Encoding failed (unsupported pixel format, tmux quirk):
                // treat it like a broken download so we stop retrying.
                self.failed.insert(slot.url.clone());
                return false;
            }
        };
        if self.protocols.len() >= PROTOCOL_CACHE_CAP {
            self.protocols.clear();
        }
        self.protocols.insert(key, proto);
        true
    }
}

impl Default for ImageStore {
    fn default() -> Self {
        Self::new()
    }
}

impl ImageResolver for ImageStore {
    /// Pixels for `path` if we have them; otherwise queue the download and
    /// let the renderer emit the `[image: …]` fallback for now.
    ///
    /// The clone is of the already-downscaled image (see
    /// [`ImageStore::decode_bytes`]); `ratatui-markdown` needs an owned
    /// `DynamicImage` and clones it once more into the placement.
    ///
    /// A zero-sized picture is reported as unresolvable on purpose: it would
    /// make [`Self::cell_dimensions`] return `(0, 0)`, and the renderer then
    /// consumes the resolved entry *without* emitting a placement — which
    /// would shift the placement↔URL pairing the caller relies on.
    fn resolve(&mut self, path: &str) -> Option<DynamicImage> {
        if self.picker().is_none() {
            return None;
        }
        if let Some(img) = self.decoded.get(path) {
            if img.width() == 0 || img.height() == 0 {
                return None;
            }
            return Some((**img).clone());
        }
        if !self.failed.contains(path)
            && !self.inflight.contains(path)
            && !self.wanted.iter().any(|u| u == path)
        {
            self.wanted.push(path.to_string());
        }
        None
    }

    /// How many cells the picture gets: its natural size at the terminal's
    /// cell resolution, shrunk (aspect-preserving) until it fits both the
    /// column width and the configured maximum height.
    fn cell_dimensions(
        &mut self,
        img: &DynamicImage,
        max_width: u16,
        _max_height: u16,
    ) -> (u16, u16) {
        let (fw, fh) = self.font_size();
        let (pw, ph) = (img.width(), img.height());
        if pw == 0 || ph == 0 || max_width == 0 {
            return (0, 0);
        }
        let nat_w = pw.div_ceil(fw as u32).max(1);
        let nat_h = ph.div_ceil(fh as u32).max(1);
        let cap_h = self.max_height() as u32;
        let cap_w = max_width as u32;

        // Shrink by whichever axis is the tighter constraint. Rounding down
        // (then clamping to 1) keeps the picture inside both caps.
        let w = if nat_w > cap_w { cap_w } else { nat_w };
        let h = (nat_h * w / nat_w).max(1);
        let (w, h) = if h > cap_h {
            ((nat_w * cap_h / nat_h).max(1).min(cap_w), cap_h)
        } else {
            (w, h)
        };
        (w as u16, h as u16)
    }
}

impl ImagePainter for ImageStore {
    fn paint(&mut self, draw: &ImageDraw, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 || draw.x >= area.width {
            return;
        }
        if !self.ensure_protocol(draw.key) {
            return;
        }
        // `SlicedImage` clips itself against `area`, including a negative y
        // (picture scrolled partly above the viewport), so no cropping math
        // is needed here — only the i16 narrowing the API asks for.
        let y = draw.y.clamp(i16::MIN as i32, i16::MAX as i32) as i16;
        let position = SignedPosition {
            x: draw.x as i16,
            y,
        };
        let Some(proto) = self.protocols.get(&draw.key) else {
            return;
        };
        SlicedImage::new(proto, position).render(area, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, RgbaImage};

    /// A store with a known, terminal-independent cell size: `halfblocks`
    /// hardcodes 10x20 px per cell and needs no terminal to query.
    fn store(max_height: u16) -> ImageStore {
        ImageStore::with_picker(Some(Picker::halfblocks()), max_height)
    }

    fn img(w: u32, h: u32) -> DynamicImage {
        DynamicImage::ImageRgba8(RgbaImage::new(w, h))
    }

    #[test]
    fn a_disabled_store_never_queues_anything() {
        let mut s = ImageStore::with_picker(None, 20);
        assert!(!s.enabled());
        assert!(s.resolve("https://host/a.png").is_none());
        assert!(s.take_wanted().is_empty());
    }

    #[test]
    fn a_miss_is_queued_once_and_only_once() {
        let mut s = store(20);
        assert!(s.resolve("u1").is_none());
        assert!(s.resolve("u1").is_none());
        assert_eq!(s.take_wanted(), vec!["u1".to_string()]);
        // Now in flight: a further miss must not re-queue it.
        assert!(s.resolve("u1").is_none());
        assert!(s.take_wanted().is_empty());
        assert!(s.has_inflight());
    }

    #[test]
    fn a_failed_url_is_never_retried() {
        let mut s = store(20);
        let _ = s.resolve("bad");
        let _ = s.take_wanted();
        assert!(!s.insert_decoded("bad", None));
        assert!(!s.has_inflight());
        assert!(s.resolve("bad").is_none());
        assert!(s.take_wanted().is_empty(), "must not queue a known failure");
    }

    #[test]
    fn a_decoded_url_resolves_and_asks_for_a_rebuild() {
        let mut s = store(20);
        let _ = s.resolve("u1");
        let _ = s.take_wanted();
        assert!(s.insert_decoded("u1", Some(Arc::new(img(40, 60)))));
        let got = s.resolve("u1").expect("resolves from cache");
        assert_eq!((got.width(), got.height()), (40, 60));
    }

    #[test]
    fn natural_size_is_used_when_it_fits() {
        let mut s = store(20);
        // 10x20 px per cell → 100x80 px is 10x4 cells.
        assert_eq!(s.cell_dimensions(&img(100, 80), 40, 0), (10, 4));
    }

    #[test]
    fn a_wide_picture_shrinks_to_the_column_width() {
        let mut s = store(40);
        // 40 x 8 cells natural, column is 20 wide → half size, aspect kept.
        let (w, h) = s.cell_dimensions(&img(400, 160), 20, 0);
        assert_eq!(w, 20);
        assert_eq!(h, 4);
    }

    #[test]
    fn a_tall_picture_shrinks_to_the_configured_max_height() {
        let mut s = store(5);
        // 20 x 20 cells natural, capped at 5 rows → 5 columns.
        let (w, h) = s.cell_dimensions(&img(200, 400), 80, 0);
        assert_eq!(h, 5);
        assert_eq!(w, 5);
    }

    #[test]
    fn a_degenerate_picture_reserves_nothing() {
        let mut s = store(20);
        assert_eq!(s.cell_dimensions(&img(0, 0), 40, 0), (0, 0));
        assert_eq!(s.cell_dimensions(&img(10, 10), 0, 0), (0, 0));
    }

    #[test]
    fn the_same_url_and_size_share_a_key_but_a_resize_does_not() {
        let mut s = store(20);
        let a = s.register("u1", 10, 4);
        let b = s.register("u1", 10, 4);
        let c = s.register("u1", 8, 4);
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn line_refs_cover_the_picture_and_number_their_rows() {
        let mut s = store(20);
        let key = s.register("u1", 12, 3);
        let refs = s.line_refs(key, 2);
        assert_eq!(refs.len(), 3);
        for (i, r) in refs.iter().enumerate() {
            assert_eq!(r.key, key);
            assert_eq!(r.col, 2);
            assert_eq!((r.width, r.height), (12, 3));
            assert_eq!(r.row_in_image, i as u16);
        }
        assert!(s.line_refs(key + 1, 0).is_empty(), "unknown key: no lines");
    }

    #[test]
    fn decode_downscales_an_oversized_picture_but_keeps_the_aspect() {
        // 2000x2000 px with a 5-row cap and 20 px cells → at most 200 px tall.
        let mut png = Vec::new();
        img(2000, 2000)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .expect("encode fixture");
        let decoded = ImageStore::decode_bytes(&png, 5, (10, 20)).expect("decodes");
        assert!(decoded.height() <= 200, "height {}", decoded.height());
        assert_eq!(decoded.width(), decoded.height(), "aspect kept");
    }

    #[test]
    fn undecodable_bytes_are_rejected_rather_than_panicking() {
        assert!(ImageStore::decode_bytes(b"not an image", 20, (10, 20)).is_none());
    }

    #[test]
    fn painting_an_unknown_or_undecoded_key_is_a_no_op() {
        let mut s = store(20);
        let key = s.register("u1", 4, 2);
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 10));
        let before = buf.clone();
        let draw = ImageDraw {
            key,
            x: 0,
            y: 0,
            width: 4,
            height: 2,
        };
        // No pixels yet.
        s.paint(&draw, Rect::new(0, 0, 20, 10), &mut buf);
        assert_eq!(buf, before);
        // Unknown key.
        s.paint(
            &ImageDraw {
                key: key + 1,
                ..draw
            },
            Rect::new(0, 0, 20, 10),
            &mut buf,
        );
        assert_eq!(buf, before);
    }

    #[test]
    fn painting_outside_the_area_is_a_no_op() {
        let mut s = store(20);
        let key = s.register("u1", 4, 2);
        s.insert_decoded("u1", Some(Arc::new(img(40, 40))));
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 10));
        let before = buf.clone();
        let draw = ImageDraw {
            key,
            x: 0,
            y: 0,
            width: 4,
            height: 2,
        };
        // Zero-sized area, and an x beyond the right edge.
        s.paint(&draw, Rect::new(0, 0, 0, 0), &mut buf);
        s.paint(
            &ImageDraw { x: 30, ..draw },
            Rect::new(0, 0, 20, 10),
            &mut buf,
        );
        assert_eq!(buf, before);
    }

    #[test]
    fn fresh_pixels_invalidate_a_protocol_built_from_the_old_ones() {
        let mut s = store(20);
        let key = s.register("u1", 4, 2);
        s.insert_decoded("u1", Some(Arc::new(img(40, 40))));
        assert!(s.ensure_protocol(key));
        assert!(s.protocols.contains_key(&key));
        s.insert_decoded("u1", Some(Arc::new(img(80, 80))));
        assert!(
            !s.protocols.contains_key(&key),
            "the stale encoding must be dropped"
        );
    }
}
