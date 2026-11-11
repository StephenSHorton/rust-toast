//! The `rust-toast` program's protocol (PROTOCOL.md): newline-delimited
//! JSON commands in, newline-delimited JSON events out, so apps not written
//! in Rust can show cards. Behind the `cli` feature.
//!
//! [`Session`] parses a line ([`parse`]), maps it onto the library
//! (configure, set_options, post, close) through a [`Backend`], and reports
//! what happens to each card through its [`Emit`]: a button
//! ([`Event::Action`]), a click on the body ([`Event::Click`]), or the card
//! going away by itself ([`Event::Dismiss`] with a [`How`]). The program
//! uses [`Global`], the process's real cards; tests drive an
//! [`crate::Engine`] headless with a recording surface.
//!
//! Every card carries a tracker shared by its callbacks. A callback marks
//! it resolved and reports what the person did; when the last reference
//! goes (the card left the library's model and any callback finished) an
//! unresolved tracker reports how the card went: `closed` when it went
//! during a close or an options change this session made, `pushed` during
//! one of its posts (a full stack pushed it off), else `timeout`. A card
//! replaced in place by a post with its id reports nothing: the id lives
//! on.

use std::cell::Cell;
use std::fs::File;
use std::io::{BufRead, Read};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::config::{Accents, Color, Config, Kind, Palette, Scheme};
use crate::layout::{TEXT_WIDTH_DIP, THUMB_MAX_H_DIP};
use crate::note::{
    Action, CORNER_BOTTOM_LEFT, CORNER_BOTTOM_RIGHT, CORNER_TOP_LEFT, CORNER_TOP_RIGHT, Image,
    MAX_ACTIONS, Note, Options,
};

/// The protocol's version, in the `ready` event. It goes up only when a
/// command or event changes meaning; new optional fields and events don't
/// bump it.
pub const PROTOCOL: u32 = 1;

/// The longest command line read; a longer one is an error and skipped.
pub const MAX_LINE: usize = 256 * 1024;
/// Longest kept, in characters; longer text is cut.
pub const MAX_ID: usize = 128;
pub const MAX_TITLE: usize = 256;
pub const MAX_BODY: usize = 2048;
pub const MAX_LABEL: usize = 64;
pub const MAX_APP_NAME: usize = 64;
pub const MAX_PATH: usize = 4096;
/// How much of a bad command an error event echoes, in characters.
pub const MAX_ECHO: usize = 200;
/// The largest thumbnail file read.
pub const MAX_THUMB_FILE: u64 = 16 * 1024 * 1024;
/// The largest thumbnail decoded, in pixels a side.
pub const MAX_THUMB_SIDE: usize = 4096;
/// The longest `timeout_ms` (a day); longer is cut to it.
pub const MAX_TIMEOUT_MS: u64 = 24 * 60 * 60 * 1000;
/// The most cards up at once a `configure` may ask for.
pub const MAX_MAX_CARDS: usize = 50;

/// One command line.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Command {
    Configure(Box<ConfigureCmd>),
    SetOptions(OptionsCmd),
    Post(PostCmd),
    Close {
        id: String,
    },
    /// Reports the ids of the cards up ([`Event::Cards`]).
    List,
    Quit,
}

/// `configure`: the look. Fields left out keep their current value.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct ConfigureCmd {
    pub app_name: Option<String>,
    pub scheme: Option<SchemeName>,
    pub corner_radius: Option<f64>,
    pub glass: Option<f64>,
    pub stripe: Option<bool>,
    pub capture_exclude: Option<bool>,
    pub shown: Option<usize>,
    pub max_cards: Option<usize>,
    pub light: Option<PaletteCmd>,
    pub dark: Option<PaletteCmd>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SchemeName {
    Light,
    Dark,
    System,
}

/// A palette's colours as `#rrggbb`; those left out keep the current ones.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct PaletteCmd {
    pub background: Option<String>,
    pub foreground: Option<String>,
    pub muted: Option<String>,
    pub border: Option<String>,
    pub accent: Option<AccentsCmd>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct AccentsCmd {
    pub info: Option<String>,
    pub success: Option<String>,
    pub warn: Option<String>,
    pub error: Option<String>,
}

/// `set_options`: the person's settings. Fields left out keep their
/// current value.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct OptionsCmd {
    pub enabled: Option<bool>,
    pub corner: Option<String>,
    pub sound: Option<bool>,
    pub volume: Option<f64>,
    pub confetti: Option<bool>,
}

/// `post`: one card.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct PostCmd {
    /// Replaces the card up with this id; empty gets one (`auto-N`).
    pub id: Option<String>,
    pub title: String,
    pub body: String,
    pub kind: Option<KindName>,
    pub sticky: bool,
    /// Milliseconds before it fades; 0 or left out is the library's
    /// default. Ignored when sticky.
    pub timeout_ms: Option<u64>,
    /// A PNG or JPEG file shown under the body.
    pub thumbnail: Option<String>,
    /// false: no sound for this card.
    pub sound: Option<bool>,
    /// A confetti burst from this success card, whatever the options say.
    pub confetti: Option<bool>,
    /// A click on the body reports `click` (default); false makes it a
    /// dismiss, as in the library.
    pub clickable: Option<bool>,
    /// Up to two buttons; the first is primary. Extra ones are dropped.
    pub actions: Vec<ActionCmd>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KindName {
    Info,
    Success,
    #[serde(alias = "warning")]
    Warn,
    Error,
}

impl From<KindName> for Kind {
    fn from(k: KindName) -> Kind {
        match k {
            KindName::Info => Kind::Info,
            KindName::Success => Kind::Success,
            KindName::Warn => Kind::Warn,
            KindName::Error => Kind::Error,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct ActionCmd {
    /// Reported in the `action` event; empty is the button's index.
    pub id: String,
    /// The button's text; empty is the id.
    pub label: String,
}

/// How a card went without the person pressing anything on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum How {
    /// The person dismissed it: the x, a right or Control click, a sideways
    /// throw, or a body click on a card with `clickable: false`. The
    /// library doesn't tell these apart.
    User,
    /// It expired.
    Timeout,
    /// A `close` command, or `set_options` turning cards off.
    Closed,
    /// A post past `max_cards` pushed it off the stack.
    Pushed,
}

/// One event line.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    Ready {
        version: String,
        protocol: u32,
        platform: String,
        supported: bool,
    },
    /// A post took: the card's id (the one given, or the one made up).
    Posted {
        id: String,
    },
    /// A button: the card's id and the button's id.
    Action {
        id: String,
        action: String,
    },
    /// A click on the card's body.
    Click {
        id: String,
    },
    Dismiss {
        id: String,
        how: How,
    },
    /// The ids of the cards up, oldest first (`list`).
    Cards {
        ids: Vec<String>,
    },
    Error {
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        /// The start of the offending line, as text.
        #[serde(skip_serializing_if = "Option::is_none")]
        command: Option<String>,
    },
}

impl Event {
    /// The event as one JSON line, without the newline.
    pub fn to_line(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| {
            r#"{"event":"error","message":"can't encode an event"}"#.to_string()
        })
    }

    /// An error event, echoing the start of the offending line.
    pub fn error(message: impl Into<String>, line: Option<&[u8]>) -> Event {
        Event::Error {
            message: bound(&message.into(), MAX_ECHO * 2),
            id: None,
            command: line.map(echo),
        }
    }
}

/// The start of a line as text, safe to put in an event: invalid UTF-8
/// replaced, control characters dropped, cut to [`MAX_ECHO`] characters.
pub fn echo(line: &[u8]) -> String {
    let s = String::from_utf8_lossy(&line[..line.len().min(MAX_ECHO * 4)]);
    let mut out: String = s
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_ECHO)
        .collect();
    if s.chars().filter(|c| !c.is_control()).count() > MAX_ECHO {
        out.push('…');
    }
    out
}

/// s cut to n characters.
pub fn bound(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// Parses one command line (without its newline). A UTF-8 byte-order mark
/// at its start, which some Windows clients send, is skipped.
pub fn parse(line: &[u8]) -> Result<Command, String> {
    let line = line.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(line);
    serde_json::from_slice::<Command>(line).map_err(|e| {
        // serde's message names the field or variant, never the input's
        // contents beyond a short token; cut it anyway.
        bound(&format!("bad command: {e}"), MAX_ECHO)
    })
}

/// One line from r, without its newline. `Ok(None)` at the end of input,
/// `Err(prefix)` for a line longer than `max` (the rest of it is read and
/// dropped; prefix is its start, for the error event).
pub fn read_line(
    r: &mut impl BufRead,
    max: usize,
) -> std::io::Result<Option<Result<Vec<u8>, Vec<u8>>>> {
    let mut line = Vec::new();
    let mut long = false;
    let mut any = false;
    loop {
        let buf = match r.fill_buf() {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        if buf.is_empty() {
            break;
        }
        any = true;
        let (chunk, done) = match buf.iter().position(|&b| b == b'\n') {
            Some(i) => (&buf[..i], Some(i + 1)),
            None => (buf, None),
        };
        if !long {
            let room = max.saturating_sub(line.len());
            if chunk.len() > room {
                line.extend_from_slice(&chunk[..room]);
                long = true;
            } else {
                line.extend_from_slice(chunk);
            }
        }
        let used = done.unwrap_or(buf.len());
        r.consume(used);
        if done.is_some() {
            return Ok(Some(finish(line, long)));
        }
    }
    if !any {
        return Ok(None);
    }
    Ok(Some(finish(line, long)))
}

fn finish(mut line: Vec<u8>, long: bool) -> Result<Vec<u8>, Vec<u8>> {
    if line.last() == Some(&b'\r') {
        line.pop();
    }
    if long { Err(line) } else { Ok(line) }
}

/// Parses `#rrggbb` or `rrggbb`; `None` for anything else (the library's
/// [`crate::hex`] panics, being meant for literals).
pub fn parse_hex(s: &str) -> Option<Color> {
    let t = s.strip_prefix('#').unwrap_or(s);
    if t.len() != 6 || !t.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let v = u32::from_str_radix(t, 16).ok()?;
    Some(crate::config::rgb((v >> 16) as u8, (v >> 8) as u8, v as u8))
}

impl ConfigureCmd {
    /// c with this command's fields applied; an error names the first bad
    /// field and nothing is applied.
    pub fn apply(&self, mut c: Config) -> Result<Config, String> {
        if let Some(a) = &self.app_name {
            c.app_name = bound(a, MAX_APP_NAME);
        }
        if let Some(s) = self.scheme {
            c.scheme = match s {
                SchemeName::Light => Scheme::Light,
                SchemeName::Dark => Scheme::Dark,
                SchemeName::System => Scheme::System,
            };
        }
        if let Some(r) = self.corner_radius {
            c.corner_radius = r;
        }
        if let Some(g) = self.glass {
            c.glass = g;
        }
        if let Some(s) = self.stripe {
            c.stripe = s;
        }
        if let Some(x) = self.capture_exclude {
            c.capture_exclude = x;
        }
        if let Some(n) = self.shown {
            c.motion.shown = n.min(MAX_MAX_CARDS);
        }
        if let Some(n) = self.max_cards {
            c.motion.max_cards = n.min(MAX_MAX_CARDS);
        }
        if let Some(p) = &self.light {
            c.light = p.apply(c.light, "light")?;
        }
        if let Some(p) = &self.dark {
            c.dark = p.apply(c.dark, "dark")?;
        }
        Ok(c)
    }
}

impl PaletteCmd {
    fn apply(&self, mut p: Palette, name: &str) -> Result<Palette, String> {
        let set = |c: &mut Color, v: &Option<String>, field: &str| -> Result<(), String> {
            if let Some(v) = v {
                *c = parse_hex(v).ok_or_else(|| format!("{name}.{field}: not a #rrggbb colour"))?;
            }
            Ok(())
        };
        set(&mut p.background, &self.background, "background")?;
        set(&mut p.foreground, &self.foreground, "foreground")?;
        set(&mut p.muted, &self.muted, "muted")?;
        set(&mut p.border, &self.border, "border")?;
        if let Some(a) = &self.accent {
            let Accents {
                info,
                success,
                warn,
                error,
            } = &mut p.accent;
            set(info, &a.info, "accent.info")?;
            set(success, &a.success, "accent.success")?;
            set(warn, &a.warn, "accent.warn")?;
            set(error, &a.error, "accent.error")?;
        }
        // The derived colours follow the new ones.
        p.faint = Color(0);
        p.hover = Color(0);
        p.button = Color(0);
        p.primary = Color(0);
        p.primary_hover = Color(0);
        p.on_primary = Color(0);
        p.tint = Color(0);
        Ok(p.resolved())
    }
}

impl OptionsCmd {
    /// o with this command's fields applied; an error names the bad field.
    pub fn apply(&self, mut o: Options) -> Result<Options, String> {
        if let Some(c) = &self.corner {
            if !matches!(
                c.as_str(),
                CORNER_TOP_RIGHT | CORNER_BOTTOM_RIGHT | CORNER_TOP_LEFT | CORNER_BOTTOM_LEFT
            ) {
                return Err(
                    "corner: one of top-right, bottom-right, top-left, bottom-left".to_string(),
                );
            }
            o.corner = c.clone();
        }
        if let Some(v) = self.volume {
            if !v.is_finite() {
                return Err("volume: a number from 0 to 1".to_string());
            }
            o.volume = v;
        }
        if let Some(e) = self.enabled {
            o.enabled = e;
        }
        if let Some(s) = self.sound {
            o.sound = s;
        }
        if let Some(c) = self.confetti {
            o.confetti = c;
        }
        Ok(o.normalized())
    }
}

/// Reads a PNG or JPEG thumbnail and shrinks it for the card's slot. Only
/// a regular file no bigger than [`MAX_THUMB_FILE`] is read, and only as
/// an image: anything that isn't a PNG or JPEG by its first bytes is
/// refused. Errors say what went wrong, never what the file holds.
pub fn load_thumbnail(path: &Path) -> Result<Image, String> {
    let meta = std::fs::metadata(path).map_err(|_| "thumbnail: can't open the file".to_string())?;
    if !meta.is_file() {
        return Err("thumbnail: not a regular file".into());
    }
    if meta.len() > MAX_THUMB_FILE {
        return Err("thumbnail: file too large".into());
    }
    // Checked again on the handle: the path may have been swapped since.
    // Non-blocking, so a pipe swapped in can't hold the open up.
    let f = open_nonblocking(path).map_err(|_| "thumbnail: can't open the file".to_string())?;
    let meta = f
        .metadata()
        .map_err(|_| "thumbnail: can't open the file".to_string())?;
    if !meta.is_file() {
        return Err("thumbnail: not a regular file".into());
    }
    let mut bytes = Vec::with_capacity(meta.len().min(MAX_THUMB_FILE) as usize);
    f.take(MAX_THUMB_FILE + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "thumbnail: can't read the file".to_string())?;
    if bytes.len() as u64 > MAX_THUMB_FILE {
        return Err("thumbnail: file too large".into());
    }
    let (w, h, rgba) = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        decode_png(&bytes)?
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        decode_jpeg(&bytes)?
    } else {
        return Err("thumbnail: not a PNG or JPEG".into());
    };
    let (w, h, rgba) = shrink(w, h, rgba);
    Image::new(w as u32, h as u32, rgba).ok_or_else(|| "thumbnail: empty image".to_string())
}

/// Opens path for reading; on macOS and Linux with O_NONBLOCK, which a
/// regular file ignores.
fn open_nonblocking(path: &Path) -> std::io::Result<File> {
    let mut o = std::fs::OpenOptions::new();
    o.read(true);
    #[cfg(any(
        target_os = "macos",
        all(
            target_os = "linux",
            any(target_arch = "x86_64", target_arch = "aarch64")
        )
    ))]
    {
        use std::os::unix::fs::OpenOptionsExt;
        #[cfg(target_os = "macos")]
        const O_NONBLOCK: i32 = 0x0004;
        #[cfg(target_os = "linux")]
        const O_NONBLOCK: i32 = 0o4000;
        o.custom_flags(O_NONBLOCK);
    }
    o.open(path)
}

// chunks_exact, not as_chunks (Rust 1.88), so older toolchains still
// build; clippy 1.99's chunks_exact_to_as_chunks prefers the latter.
#[allow(unknown_lints, clippy::chunks_exact_to_as_chunks)]
fn decode_png(bytes: &[u8]) -> Result<(usize, usize, Vec<u8>), String> {
    let bad = |_| "thumbnail: not a readable PNG".to_string();
    let mut d = png::Decoder::new_with_limits(
        bytes,
        png::Limits {
            bytes: MAX_THUMB_SIDE * MAX_THUMB_SIDE * 4,
        },
    );
    d.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut r = d.read_info().map_err(bad)?;
    let (w, h) = (r.info().width as usize, r.info().height as usize);
    if w == 0 || h == 0 || w > MAX_THUMB_SIDE || h > MAX_THUMB_SIDE {
        return Err("thumbnail: image too large".into());
    }
    let mut buf = vec![0; r.output_buffer_size()];
    let f = r.next_frame(&mut buf).map_err(bad)?;
    let px = &buf[..f.buffer_size()];
    let (fw, fh) = (f.width as usize, f.height as usize);
    let rgba: Vec<u8> = match f.color_type {
        png::ColorType::Rgba => px.to_vec(),
        png::ColorType::Rgb => px
            .chunks_exact(3)
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect(),
        png::ColorType::GrayscaleAlpha => px
            .chunks_exact(2)
            .flat_map(|p| [p[0], p[0], p[0], p[1]])
            .collect(),
        png::ColorType::Grayscale => px.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::Indexed => return Err("thumbnail: unsupported PNG colours".into()),
    };
    if rgba.len() != fw * fh * 4 {
        return Err("thumbnail: unsupported PNG layout".into());
    }
    Ok((fw, fh, rgba))
}

fn decode_jpeg(bytes: &[u8]) -> Result<(usize, usize, Vec<u8>), String> {
    use zune_jpeg::JpegDecoder;
    use zune_jpeg::zune_core::colorspace::ColorSpace;
    use zune_jpeg::zune_core::options::DecoderOptions;
    let opts = DecoderOptions::default()
        .jpeg_set_out_colorspace(ColorSpace::RGBA)
        .set_max_width(MAX_THUMB_SIDE)
        .set_max_height(MAX_THUMB_SIDE);
    let mut d = JpegDecoder::new_with_options(bytes, opts);
    let px = d
        .decode()
        .map_err(|_| "thumbnail: not a readable JPEG".to_string())?;
    let (w, h) = d
        .dimensions()
        .ok_or_else(|| "thumbnail: not a readable JPEG".to_string())?;
    if w == 0 || h == 0 || px.len() != w * h * 4 {
        return Err("thumbnail: unsupported JPEG layout".into());
    }
    Ok((w, h, px))
}

/// Shrinks an image (box filter) so neither side is more than four times
/// the thumbnail slot's: the card still draws it at the slot's size (it
/// keeps the aspect), with pixels to spare on a high-density screen.
pub fn shrink(w: usize, h: usize, rgba: Vec<u8>) -> (usize, usize, Vec<u8>) {
    let (mw, mh) = (TEXT_WIDTH_DIP * 4.0, THUMB_MAX_H_DIP * 4.0);
    let f = (w as f64 / mw).max(h as f64 / mh);
    if f <= 1.0 {
        return (w, h, rgba);
    }
    let nw = ((w as f64 / f).round() as usize).max(1);
    let nh = ((h as f64 / f).round() as usize).max(1);
    let mut out = vec![0u8; nw * nh * 4];
    for y in 0..nh {
        let (y0, y1) = (y * h / nh, ((y + 1) * h / nh).max(y * h / nh + 1));
        for x in 0..nw {
            let (x0, x1) = (x * w / nw, ((x + 1) * w / nw).max(x * w / nw + 1));
            let mut acc = [0u64; 4];
            for sy in y0..y1 {
                for sx in x0..x1 {
                    let i = (sy * w + sx) * 4;
                    for (c, a) in acc.iter_mut().enumerate() {
                        *a += rgba[i + c] as u64;
                    }
                }
            }
            let n = ((y1 - y0) * (x1 - x0)) as u64;
            let o = (y * nw + x) * 4;
            for c in 0..4 {
                out[o + c] = (acc[c] / n) as u8;
            }
        }
    }
    (nw, nh, out)
}

/// Where events go. Called from any thread, also the cards' own; it must
/// not block for long. (The `rust-toast` program's may wait on the thread
/// that runs commands, for the app to read, never on the cards' threads.)
pub type Emit = Arc<dyn Fn(Event) + Send + Sync>;

/// What the session drives: the library's free functions ([`Global`]) or,
/// in tests, an engine.
pub trait Backend {
    /// Whether this system draws cards.
    fn supported(&self) -> bool;
    fn configure(&self, c: Config);
    fn set_options(&self, o: Options);
    fn options(&self) -> Options;
    fn post(&self, n: Note);
    fn close(&self, id: &str);
}

/// The process's cards: [`crate::post`] and friends.
pub struct Global;

impl Backend for Global {
    fn supported(&self) -> bool {
        crate::platform::DRAWS
    }
    fn configure(&self, c: Config) {
        crate::configure(c)
    }
    fn set_options(&self, o: Options) {
        crate::set_options(o)
    }
    fn options(&self) -> Options {
        crate::current_options()
    }
    fn post(&self, n: Note) {
        crate::post(n)
    }
    fn close(&self, id: &str) {
        crate::close(id)
    }
}

/// The platform's name, as the `ready` event reports it.
pub fn platform_name() -> &'static str {
    if cfg!(windows) {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else {
        "other"
    }
}

/// What this thread is doing to the cards, so a tracker dropped meanwhile
/// knows why its card went.
#[derive(Clone, Copy)]
enum Doing {
    Nothing,
    /// Posting the card with this generation.
    Posting(u64),
    Closing,
}

thread_local! {
    static DOING: Cell<Doing> = const { Cell::new(Doing::Nothing) };
}

fn doing<T>(d: Doing, f: impl FnOnce() -> T) -> T {
    struct Reset(Doing);
    impl Drop for Reset {
        fn drop(&mut self) {
            DOING.with(|c| c.set(self.0));
        }
    }
    let _reset = Reset(DOING.with(|c| c.replace(d)));
    f()
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The cards up: id to the generation of its latest post, in post order.
#[derive(Default)]
struct Live {
    cards: Vec<(String, u64)>,
}

impl Live {
    fn serial_of(&self, id: &str) -> Option<u64> {
        self.cards.iter().find(|(i, _)| i == id).map(|(_, g)| *g)
    }
}

/// Shared by one card's callbacks; reports how the card went when the
/// last of them goes.
struct Tracker {
    id: String,
    serial: u64,
    resolved: AtomicBool,
    emit: Emit,
    live: Arc<Mutex<Live>>,
}

impl Tracker {
    /// The person did something: the first report wins.
    fn resolve(&self, e: Event) {
        if !self.resolved.swap(true, Ordering::AcqRel) {
            (self.emit)(e);
        }
    }
}

impl Drop for Tracker {
    fn drop(&mut self) {
        let current = {
            let mut live = lock(&self.live);
            let current = live.serial_of(&self.id) == Some(self.serial);
            if current {
                live.cards.retain(|(i, _)| i != &self.id);
            }
            current
        };
        if !current || self.resolved.load(Ordering::Acquire) {
            return; // replaced in place, or the person's report went out
        }
        let how = match DOING.with(|c| c.get()) {
            Doing::Posting(g) if g == self.serial => {
                (self.emit)(Event::Error {
                    message: "not shown: cards are off or the note is empty".into(),
                    id: Some(self.id.clone()),
                    command: None,
                });
                return;
            }
            Doing::Posting(_) => How::Pushed,
            Doing::Closing => How::Closed,
            Doing::Nothing => How::Timeout,
        };
        (self.emit)(Event::Dismiss {
            id: self.id.clone(),
            how,
        });
    }
}

/// What to do after a line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    Go,
    Quit,
}

/// One program's commands applied to a backend.
pub struct Session<B: Backend> {
    backend: B,
    emit: Emit,
    config: Config,
    live: Arc<Mutex<Live>>,
    serial: u64,
    auto: u64,
}

impl<B: Backend> Session<B> {
    /// A session starting from `config` (sent to the backend at once).
    pub fn new(backend: B, config: Config, emit: Emit) -> Session<B> {
        if backend.supported() {
            backend.configure(config.clone());
        }
        Session {
            backend,
            emit,
            config,
            live: Arc::default(),
            serial: 0,
            auto: 0,
        }
    }

    pub fn backend(&self) -> &B {
        &self.backend
    }

    /// The `ready` event.
    pub fn ready(&self) -> Event {
        Event::Ready {
            version: env!("CARGO_PKG_VERSION").into(),
            protocol: PROTOCOL,
            platform: platform_name().into(),
            supported: self.backend.supported(),
        }
    }

    /// The ids of the cards up, oldest first.
    pub fn ids(&self) -> Vec<String> {
        lock(&self.live)
            .cards
            .iter()
            .map(|(i, _)| i.clone())
            .collect()
    }

    /// Handles one raw line: parses it, runs it, reports errors as events.
    pub fn line(&mut self, line: &[u8]) -> Flow {
        if line.iter().all(|b| b.is_ascii_whitespace()) {
            return Flow::Go;
        }
        match parse(line) {
            Ok(c) => match self.run(c) {
                Ok(f) => f,
                Err((message, id)) => {
                    (self.emit)(Event::Error {
                        message,
                        id,
                        command: Some(echo(line)),
                    });
                    Flow::Go
                }
            },
            Err(m) => {
                (self.emit)(Event::error(m, Some(line)));
                Flow::Go
            }
        }
    }

    /// Runs one command. An error carries a message and the card's id.
    pub fn run(&mut self, c: Command) -> Result<Flow, (String, Option<String>)> {
        let on = self.backend.supported();
        match c {
            Command::Quit => return Ok(Flow::Quit),
            Command::List => (self.emit)(Event::Cards { ids: self.ids() }),
            Command::Configure(c) => {
                let next = c.apply(self.config.clone()).map_err(|e| (e, None))?;
                self.config = next;
                if on {
                    self.backend.configure(self.config.clone());
                }
            }
            Command::SetOptions(o) => {
                if on {
                    let next = o.apply(self.backend.options()).map_err(|e| (e, None))?;
                    doing(Doing::Closing, || self.backend.set_options(next));
                } else {
                    o.apply(Options::default()).map_err(|e| (e, None))?;
                }
            }
            Command::Close { id } => {
                let id = bound(&id, MAX_ID);
                if on {
                    doing(Doing::Closing, || self.backend.close(&id));
                }
            }
            Command::Post(p) => self.post(p, on)?,
        }
        Ok(Flow::Go)
    }

    fn post(&mut self, p: PostCmd, on: bool) -> Result<(), (String, Option<String>)> {
        let id = match p.id.as_deref().map(|i| bound(i.trim(), MAX_ID)) {
            Some(i) if !i.is_empty() => i,
            _ => {
                self.auto += 1;
                format!("auto-{}", self.auto)
            }
        };
        let err = |m: &str| (m.to_string(), Some(id.clone()));
        let title = bound(p.title.trim(), MAX_TITLE);
        let body = bound(p.body.trim(), MAX_BODY);
        if title.is_empty() && body.is_empty() {
            return Err(err("post: a title or a body is needed"));
        }
        if !on {
            (self.emit)(Event::Posted { id });
            return Ok(());
        }
        let opts = self.backend.options();
        if !opts.enabled {
            return Err(err("not shown: cards are off (set_options enabled)"));
        }
        let image = match &p.thumbnail {
            Some(t) if !t.is_empty() => {
                if t.chars().count() > MAX_PATH {
                    (self.emit)(Event::Error {
                        message: "thumbnail: path too long".into(),
                        id: Some(id.clone()),
                        command: None,
                    });
                    None
                } else {
                    match load_thumbnail(Path::new(t)) {
                        Ok(i) => Some(i),
                        Err(m) => {
                            // The card still goes up, without the picture.
                            (self.emit)(Event::Error {
                                message: m,
                                id: Some(id.clone()),
                                command: None,
                            });
                            None
                        }
                    }
                }
            }
            _ => None,
        };
        self.serial += 1;
        let serial = self.serial;
        let t = Arc::new(Tracker {
            id: id.clone(),
            serial,
            resolved: AtomicBool::new(false),
            emit: self.emit.clone(),
            live: self.live.clone(),
        });
        let actions = p
            .actions
            .iter()
            .take(MAX_ACTIONS)
            .enumerate()
            .map(|(i, a)| {
                let aid = match bound(a.id.trim(), MAX_ID) {
                    s if s.is_empty() => i.to_string(),
                    s => s,
                };
                let label = match bound(a.label.trim(), MAX_LABEL) {
                    s if s.is_empty() => bound(&aid, MAX_LABEL),
                    s => s,
                };
                let t = t.clone();
                let (nid, action) = (id.clone(), aid);
                Action::new(label, move || {
                    t.resolve(Event::Action {
                        id: nid.clone(),
                        action: action.clone(),
                    })
                })
            })
            .collect();
        let on_click = (p.clickable != Some(false)).then(|| {
            let (t, nid) = (t.clone(), id.clone());
            Arc::new(move || t.resolve(Event::Click { id: nid.clone() })) as crate::Callback
        });
        let on_dismiss = {
            let (t, nid) = (t.clone(), id.clone());
            Some(Arc::new(move || {
                t.resolve(Event::Dismiss {
                    id: nid.clone(),
                    how: How::User,
                })
            }) as crate::Callback)
        };
        drop(t);
        let note = Note {
            id: id.clone(),
            title,
            body,
            image,
            kind: p.kind.map(Kind::from).unwrap_or_default(),
            silent: p.sound == Some(false),
            expire: Duration::from_millis(p.timeout_ms.unwrap_or(0).min(MAX_TIMEOUT_MS)),
            sticky: p.sticky,
            actions,
            on_click,
            on_dismiss,
        };
        {
            let mut live = lock(&self.live);
            match live.cards.iter_mut().find(|(i, _)| i == &id) {
                Some(c) => c.1 = serial, // a replace keeps its place
                None => live.cards.push((id.clone(), serial)),
            }
        }
        let confetti = p.confetti.filter(|c| *c != opts.confetti);
        doing(Doing::Posting(serial), || {
            if let Some(c) = confetti {
                self.backend.set_options(Options {
                    confetti: c,
                    ..opts.clone()
                });
            }
            self.backend.post(note);
            if confetti.is_some() {
                self.backend.set_options(opts);
            }
        });
        if lock(&self.live).serial_of(&id) == Some(serial) {
            (self.emit)(Event::Posted { id });
        }
        Ok(())
    }
}
