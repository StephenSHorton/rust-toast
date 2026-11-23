//! The shared stack: every program on the computer that uses rust-toast
//! and shows its cards in the same corner shows them in one stack, the way
//! rsz-caffeine's shared switch is one switch for every program.
//!
//! One program per corner draws the stack (the host); the others (clients)
//! send it their cards and get back what the person did with them.
//!
//! - **Who draws.** The host holds an OS lock on `<corner>.lock` in the
//!   share folder ([`default_dir`]) for as long as it hosts; the OS drops
//!   the lock when the program quits, however it quits. The host listens on
//!   a loopback port and writes the port and a random token to
//!   `<corner>.addr`. The folder is the person's own, so only their
//!   programs can read the token, and the host takes no connection without
//!   it.
//! - **Who joins.** A program's first card in a corner tries the lock: got
//!   it, it hosts; held, it reads the address and connects. If neither
//!   works it draws its own cards as before (and tries again later).
//! - **The cards.** A client keeps its notes (with their callbacks) and
//!   sends each card's words, picture, kind, buttons and its own look. The
//!   host draws it in that look in its stack (hover, the deck, Clear all and
//!   expiry are the host's) and reports a button, a click, a dismiss, or the
//!   card going away by itself; the client runs its own callback. Each app
//!   plays its own sounds at its own volume.
//! - **Hand-over.** When the host quits, its clients see the link close,
//!   one of them wins the lock, and every client sends its live cards again
//!   (with the time each had left). A host whose person turns cards off or
//!   moves them to another corner gives the corner up the same way.
//!
//! The folder name carries the wire version ([`WIRE`]), so programs built
//! with different rust-toast versions never join one stack. Tests point
//! [`DIR_ENV`] at a folder of their own.

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions, TryLockError};
use std::hash::{BuildHasher, Hasher};
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crate::config::{Accents, Color, Config, Kind, Palette, Scheme};
use crate::engine::{Engine, SHARED_MARK, run_callback};
use crate::note::{Action, Callback, Image, Note, Options, clean, default_lifetime};
use crate::sound::{sound_bytes, sound_for};

/// The wire version: in the share folder's name and the hello.
pub const WIRE: u32 = 1;
/// An environment variable naming the folder the share folder goes in,
/// instead of the person's app data (tests).
pub const DIR_ENV: &str = "RUST_TOAST_SHARE_DIR";
/// The largest frame read (a card with its picture).
pub const MAX_FRAME: usize = 64 << 20;
/// The largest picture sent with a card (bytes of RGBA); a bigger one is
/// left off it.
const MAX_IMAGE: usize = MAX_FRAME - (1 << 20);
/// How long a connect, a hello or a write may take.
const LINK_TIMEOUT: Duration = Duration::from_millis(1500);
const CONNECT_TIMEOUT: Duration = Duration::from_millis(250);
/// Tries at the lock and the address before drawing alone.
const TRIES: usize = 10;
const TRY_GAP: Duration = Duration::from_millis(40);
/// How long a program that had to draw alone waits before trying again.
const SOLO_RETRY: Duration = Duration::from_secs(30);

/// The share folder: `%LOCALAPPDATA%\rust-toast\share-v1` on Windows,
/// `~/Library/Application Support/rust-toast/share-v1` on macOS, or
/// `share-v1` in [`DIR_ENV`] when it is set. `None` elsewhere.
pub fn default_dir() -> Option<PathBuf> {
    let base = if let Some(d) = std::env::var_os(DIR_ENV).filter(|d| !d.is_empty()) {
        PathBuf::from(d)
    } else if cfg!(windows) {
        PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("rust-toast")
    } else if cfg!(target_os = "macos") {
        PathBuf::from(std::env::var_os("HOME")?).join("Library/Application Support/rust-toast")
    } else {
        return None;
    };
    Some(base.join(format!("share-v{WIRE}")))
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

// ---- The wire ----------------------------------------------------------

/// A card on the wire.
#[derive(Debug, Clone, PartialEq)]
pub struct PostMsg {
    /// The client's number for this post; events name it.
    pub serial: u64,
    pub id: String,
    pub title: String,
    pub body: String,
    pub kind: Kind,
    pub sticky: bool,
    /// Time left before it fades (0: the library's default for it).
    pub expire_ms: u64,
    /// A confetti burst (a success card, the client's confetti on).
    pub burst: bool,
    /// The body has a click action.
    pub clickable: bool,
    /// Each button's label and whether it has an action (else it only
    /// closes the card).
    pub actions: Vec<(String, bool)>,
    /// (w, h, rgba).
    pub image: Option<(u32, u32, Vec<u8>)>,
}

/// One message between a client and a host.
#[derive(Debug, Clone, PartialEq)]
pub enum Msg {
    /// Client, first: the wire version, the host's token, the client's
    /// process id and look.
    Hello {
        wire: u32,
        token: String,
        pid: u32,
        look: Box<Config>,
    },
    Post(Box<PostMsg>),
    /// Takes the client's card with this id down, no callbacks.
    Close(String),
    /// Dismisses the client's cards, as if each one's x were pressed.
    DismissAll,
    /// The client's look changed: its cards restyle.
    Restyle(Box<Config>),
    /// Host, after a good hello.
    Welcome,
    /// The person pressed button i on the card posted with serial.
    Action(u64, u8),
    Click(u64),
    Dismiss(u64),
    /// The card posted with serial left the stack without the person
    /// doing anything with it (expired, replaced, pushed off, closed).
    Gone(u64),
}

#[derive(Default)]
struct W(Vec<u8>);

impl W {
    fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn u64(&mut self, v: u64) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn f64(&mut self, v: f64) {
        self.u64(v.to_bits());
    }
    fn bytes(&mut self, b: &[u8]) {
        self.u32(b.len() as u32);
        self.0.extend_from_slice(b);
    }
    fn str(&mut self, s: &str) {
        self.bytes(s.as_bytes());
    }
    fn color(&mut self, c: Color) {
        self.u32(c.0);
    }
    fn palette(&mut self, p: &Palette) {
        for c in [
            p.background,
            p.foreground,
            p.muted,
            p.border,
            p.accent.info,
            p.accent.success,
            p.accent.warn,
            p.accent.error,
            p.faint,
            p.hover,
            p.button,
            p.primary,
            p.primary_hover,
            p.on_primary,
            p.tint,
        ] {
            self.color(c);
        }
    }
    fn look(&mut self, c: &Config) {
        self.str(&c.app_name);
        self.u8(match c.scheme {
            Scheme::Light => 0,
            Scheme::Dark => 1,
            Scheme::System => 2,
        });
        self.palette(&c.light);
        self.palette(&c.dark);
        self.f64(c.corner_radius);
        self.f64(c.glass);
        self.u8(c.stripe as u8);
        self.u8(c.capture_exclude as u8);
    }
}

struct R<'a>(&'a [u8]);

impl R<'_> {
    fn take(&mut self, n: usize) -> Option<&[u8]> {
        if self.0.len() < n {
            return None;
        }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Some(a)
    }
    fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|b| b[0])
    }
    fn bool(&mut self) -> Option<bool> {
        self.u8().map(|b| b != 0)
    }
    fn u32(&mut self) -> Option<u32> {
        self.take(4)
            .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
    }
    fn u64(&mut self) -> Option<u64> {
        self.take(8)
            .map(|b| u64::from_le_bytes(b.try_into().unwrap()))
    }
    fn f64(&mut self) -> Option<f64> {
        self.u64().map(f64::from_bits)
    }
    fn bytes(&mut self) -> Option<&[u8]> {
        let n = self.u32()? as usize;
        self.take(n)
    }
    fn str(&mut self) -> Option<String> {
        String::from_utf8(self.bytes()?.to_vec()).ok()
    }
    fn color(&mut self) -> Option<Color> {
        self.u32().map(Color)
    }
    fn palette(&mut self) -> Option<Palette> {
        let mut c = [Color(0); 15];
        for v in &mut c {
            *v = self.color()?;
        }
        Some(Palette {
            background: c[0],
            foreground: c[1],
            muted: c[2],
            border: c[3],
            accent: Accents {
                info: c[4],
                success: c[5],
                warn: c[6],
                error: c[7],
            },
            faint: c[8],
            hover: c[9],
            button: c[10],
            primary: c[11],
            primary_hover: c[12],
            on_primary: c[13],
            tint: c[14],
        })
    }
    fn look(&mut self) -> Option<Config> {
        let app_name = self.str()?;
        let scheme = match self.u8()? {
            0 => Scheme::Light,
            1 => Scheme::Dark,
            2 => Scheme::System,
            _ => return None,
        };
        Some(
            Config {
                app_name,
                scheme,
                light: self.palette()?,
                dark: self.palette()?,
                corner_radius: self.f64()?,
                glass: self.f64()?,
                stripe: self.bool()?,
                capture_exclude: self.bool()?,
                ..Config::default()
            }
            .normalized(),
        )
    }
}

fn kind_byte(k: Kind) -> u8 {
    k.index() as u8
}

fn byte_kind(b: u8) -> Option<Kind> {
    Some(match b {
        0 => Kind::Info,
        1 => Kind::Success,
        2 => Kind::Warn,
        3 => Kind::Error,
        _ => return None,
    })
}

impl Msg {
    /// The message as one frame: its length (u32, little-endian), then it.
    pub fn frame(&self) -> Vec<u8> {
        let mut w = W::default();
        match self {
            Msg::Hello {
                wire,
                token,
                pid,
                look,
            } => {
                w.u8(1);
                w.u32(*wire);
                w.str(token);
                w.u32(*pid);
                w.look(look);
            }
            Msg::Post(p) => {
                w.u8(2);
                w.u64(p.serial);
                w.str(&p.id);
                w.str(&p.title);
                w.str(&p.body);
                w.u8(kind_byte(p.kind));
                w.u8(p.sticky as u8);
                w.u64(p.expire_ms);
                w.u8(p.burst as u8);
                w.u8(p.clickable as u8);
                w.u8(p.actions.len() as u8);
                for (label, run) in &p.actions {
                    w.str(label);
                    w.u8(*run as u8);
                }
                match &p.image {
                    Some((iw, ih, rgba)) => {
                        w.u8(1);
                        w.u32(*iw);
                        w.u32(*ih);
                        w.bytes(rgba);
                    }
                    None => w.u8(0),
                }
            }
            Msg::Close(id) => {
                w.u8(3);
                w.str(id);
            }
            Msg::DismissAll => w.u8(4),
            Msg::Restyle(c) => {
                w.u8(5);
                w.look(c);
            }
            Msg::Welcome => w.u8(10),
            Msg::Action(s, i) => {
                w.u8(11);
                w.u64(*s);
                w.u8(*i);
            }
            Msg::Click(s) => {
                w.u8(12);
                w.u64(*s);
            }
            Msg::Dismiss(s) => {
                w.u8(13);
                w.u64(*s);
            }
            Msg::Gone(s) => {
                w.u8(14);
                w.u64(*s);
            }
        }
        let mut out = Vec::with_capacity(w.0.len() + 4);
        out.extend_from_slice(&(w.0.len() as u32).to_le_bytes());
        out.extend_from_slice(&w.0);
        out
    }

    /// A frame's message (without its length); `None` when it is not one
    /// this version knows.
    pub fn parse(b: &[u8]) -> Option<Msg> {
        let mut r = R(b);
        let m = match r.u8()? {
            1 => Msg::Hello {
                wire: r.u32()?,
                token: r.str()?,
                pid: r.u32()?,
                look: Box::new(r.look()?),
            },
            2 => {
                let serial = r.u64()?;
                let id = r.str()?;
                let title = r.str()?;
                let body = r.str()?;
                let kind = byte_kind(r.u8()?)?;
                let sticky = r.bool()?;
                let expire_ms = r.u64()?;
                let burst = r.bool()?;
                let clickable = r.bool()?;
                let n = r.u8()?;
                let mut actions = vec![];
                for _ in 0..n {
                    actions.push((r.str()?, r.bool()?));
                }
                let image = match r.u8()? {
                    0 => None,
                    _ => Some((r.u32()?, r.u32()?, r.bytes()?.to_vec())),
                };
                Msg::Post(Box::new(PostMsg {
                    serial,
                    id,
                    title,
                    body,
                    kind,
                    sticky,
                    expire_ms,
                    burst,
                    clickable,
                    actions,
                    image,
                }))
            }
            3 => Msg::Close(r.str()?),
            4 => Msg::DismissAll,
            5 => Msg::Restyle(Box::new(r.look()?)),
            10 => Msg::Welcome,
            11 => Msg::Action(r.u64()?, r.u8()?),
            12 => Msg::Click(r.u64()?),
            13 => Msg::Dismiss(r.u64()?),
            14 => Msg::Gone(r.u64()?),
            _ => return None,
        };
        r.0.is_empty().then_some(m)
    }
}

/// Reads one frame's message. An unknown message is an error: the link is
/// one version on both ends.
pub fn read_msg(s: &mut impl Read) -> io::Result<Msg> {
    let mut n = [0; 4];
    s.read_exact(&mut n)?;
    let n = u32::from_le_bytes(n) as usize;
    if n > MAX_FRAME {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "frame too big"));
    }
    let mut b = vec![0; n];
    s.read_exact(&mut b)?;
    Msg::parse(&b).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "bad message"))
}

fn send(mut s: &TcpStream, m: &Msg) -> io::Result<()> {
    s.write_all(&m.frame())
}

// ---- The shared stack ---------------------------------------------------

/// A card this program sent to another program's stack.
struct Mine {
    id: String,
    serial: u64,
    note: Note,
    /// When it fades (`None`: sticky).
    until: Option<Instant>,
    burst: bool,
}

struct Host {
    corner: String,
    _lock: File,
    /// The listener's port: a connect to it wakes the accept loop to stop.
    port: u16,
    stop: Arc<AtomicBool>,
    conns: Arc<Mutex<HashMap<u64, TcpStream>>>,
}

struct Client {
    corner: String,
    out: TcpStream,
    /// This link's number: a reader that outlived it knows it.
    link: u64,
}

enum Role {
    /// Not in a corner yet: the next card picks.
    Idle,
    /// Draws its own cards: the share folder or the link failed.
    Solo {
        corner: String,
        since: Instant,
    },
    Host(Host),
    Client(Client),
}

impl Role {
    fn corner(&self) -> Option<&str> {
        match self {
            Role::Idle => None,
            Role::Solo { corner, .. } => Some(corner),
            Role::Host(h) => Some(&h.corner),
            Role::Client(c) => Some(&c.corner),
        }
    }
}

struct State {
    role: Role,
    mine: Vec<Mine>,
    serial: u64,
    auto: u64,
    link: u64,
}

/// A program's way into the shared stack. The library has one over its
/// cards; tests make several over engines of their own.
pub struct Share {
    engine: &'static Engine,
    dir: Option<PathBuf>,
    wake: fn(),
    play: fn(Arc<[u8]>),
    st: Mutex<State>,
}

/// The host's end of one client's link: what its callbacks report on. The
/// last reference going (the card left the stack) reports [`Msg::Gone`].
struct Report {
    out: Arc<Mutex<TcpStream>>,
    serial: u64,
    pid: u32,
}

impl Report {
    fn send(&self, m: Msg) {
        #[cfg(windows)]
        unsafe {
            // The click came to this program; let the card's own program
            // bring its window forward.
            windows_sys::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(self.pid);
        }
        #[cfg(not(windows))]
        let _ = self.pid;
        let _ = send(&lock(&self.out), &m);
    }
}

impl Drop for Report {
    fn drop(&mut self) {
        let _ = send(&lock(&self.out), &Msg::Gone(self.serial));
    }
}

fn token() -> String {
    let mut out = String::new();
    for i in 0..2u64 {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u64(i);
        h.write_u32(std::process::id());
        h.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        );
        out.push_str(&format!("{:016x}", h.finish()));
    }
    out
}

impl Share {
    /// A way into the shared stack in dir ([`default_dir`]; `None` never
    /// shares) for the cards in engine, with the platform's wake and sound.
    pub fn new(
        engine: &'static Engine,
        dir: Option<PathBuf>,
        wake: fn(),
        play: fn(Arc<[u8]>),
    ) -> Share {
        Share {
            engine,
            dir,
            wake,
            play,
            st: Mutex::new(State {
                role: Role::Idle,
                mine: vec![],
                serial: 0,
                auto: 0,
                link: 0,
            }),
        }
    }

    /// Whether this program draws the stack in its corner (tests).
    pub fn is_host(&self) -> bool {
        matches!(lock(&self.st).role, Role::Host(_))
    }

    /// Whether this program sends its cards to another's stack (tests).
    pub fn is_client(&self) -> bool {
        matches!(lock(&self.st).role, Role::Client(_))
    }

    /// The ids of the cards this program has in another's stack (tests).
    pub fn sent(&self) -> Vec<String> {
        lock(&self.st).mine.iter().map(|m| m.id.clone()).collect()
    }

    /// Sets the look; a client's cards in the shared stack restyle. True
    /// when this program's own windows should sync.
    pub fn configure(&'static self, c: Config) -> bool {
        let changed = self.engine.configure(c);
        if changed {
            let mut st = lock(&self.st);
            if let Role::Client(cl) = &st.role {
                let m = Msg::Restyle(Box::new(self.engine.config()));
                if send(&cl.out, &m).is_err() {
                    self.lost_now(&mut st);
                }
            }
        }
        changed
    }

    /// Applies the person's settings. Cards off leaves the shared stack
    /// (a host gives its corner up; a client's cards go); another corner
    /// moves to that corner's stack. True when the windows should sync.
    pub fn set_options(&'static self, o: Options) -> bool {
        let o = o.normalized();
        let mut gone = vec![];
        let mut st = lock(&self.st);
        let moved = st.role.corner().is_some_and(|c| c != o.corner);
        // Leave before the engine applies them: cards off clears its
        // stack, and other programs' cards must go back to them first
        // (closed with the link, not reported gone).
        if moved || !o.enabled {
            self.leave(&mut st);
        }
        drop(st);
        let changed = self.engine.set_options(o.clone());
        {
            let mut st = lock(&self.st);
            if moved || !o.enabled {
                if !o.enabled {
                    gone = std::mem::take(&mut st.mine);
                } else if !st.mine.is_empty() {
                    self.rejoin(&mut st, &o.corner);
                }
            }
        }
        drop(gone); // their trackers report outside the lock
        changed
    }

    /// Shows n: in this corner's shared stack, else here.
    pub fn post(&'static self, n: Note) {
        let o = self.engine.options();
        if !o.enabled {
            return;
        }
        let mut st = lock(&self.st);
        self.ensure(&mut st, &o.corner);
        if matches!(st.role, Role::Client(_)) {
            self.send_post(&mut st, n, &o);
            return;
        }
        drop(st);
        self.post_here(n);
    }

    /// Takes the card with id down, without its callbacks.
    pub fn close(&'static self, id: &str) {
        let mut gone = None;
        {
            let mut st = lock(&self.st);
            if let Some(i) = st.mine.iter().position(|m| m.id == id) {
                gone = Some(st.mine.remove(i));
                if let Role::Client(c) = &st.role
                    && send(&c.out, &Msg::Close(id.to_string())).is_err()
                {
                    self.lost_now(&mut st);
                }
            }
        }
        drop(gone);
        if self.engine.close(id) {
            (self.wake)();
        }
    }

    /// Dismisses this program's cards (not other programs' in its stack),
    /// as if each one's x were pressed.
    pub fn dismiss_all(&'static self) {
        {
            let mut st = lock(&self.st);
            if let Role::Client(c) = &st.role
                && !st.mine.is_empty()
                && send(&c.out, &Msg::DismissAll).is_err()
            {
                self.lost_now(&mut st);
            }
        }
        if self.engine.dismiss_own() {
            (self.wake)();
        }
    }

    /// Whether any of this program's cards is up, here or in another's
    /// stack.
    pub fn visible(&self) -> bool {
        self.engine.visible() || !lock(&self.st).mine.is_empty()
    }

    fn post_here(&self, n: Note) {
        let p = self.engine.post(n, Instant::now());
        if let Some((s, vol)) = p.sound
            && let Some(b) = sound_bytes(s, vol)
        {
            (self.play)(b);
        }
        if p.shown {
            (self.wake)();
        }
    }

    fn send_post(&'static self, st: &mut State, n: Note, o: &Options) {
        let Some(mut n) = clean(n) else { return };
        if n.id.is_empty() {
            st.auto += 1;
            n.id = format!("\u{2}auto-{}", st.auto);
        }
        if !n.silent
            && o.sound
            && let Some(b) = sound_bytes(sound_for(n.kind, !n.actions.is_empty()), o.volume)
        {
            (self.play)(b);
        }
        st.serial += 1;
        let life = default_lifetime(&n);
        let m = Mine {
            id: n.id.clone(),
            serial: st.serial,
            until: (life > Duration::ZERO).then(|| Instant::now() + life),
            burst: o.confetti && n.kind == Kind::Success,
            note: n,
        };
        let frame = post_msg(&m, Instant::now());
        let old = match st.mine.iter().position(|x| x.id == m.id) {
            Some(i) => Some(std::mem::replace(&mut st.mine[i], m)),
            None => {
                st.mine.push(m);
                None
            }
        };
        if let (Role::Client(c), Some(f)) = (&st.role, frame)
            && send(&c.out, &f).is_err()
        {
            self.lost_now(st);
        }
        drop(old);
    }

    /// Picks this program's part in corner when it has none (or drew alone
    /// long enough ago to try again).
    fn ensure(&'static self, st: &mut State, corner: &str) {
        let pick = match &st.role {
            Role::Idle => true,
            Role::Solo { corner: c, since } => c != corner || since.elapsed() >= SOLO_RETRY,
            r => r.corner() != Some(corner),
        };
        if pick {
            self.leave(st);
            st.role = self.elect(st, corner);
        }
    }

    /// Leaves the stack this program is in: a host gives the corner up
    /// (other programs' cards go; they come back in the next host's
    /// stack), a client drops its link (its cards stay in `mine`).
    fn leave(&self, st: &mut State) {
        match std::mem::replace(&mut st.role, Role::Idle) {
            Role::Host(h) => {
                h.stop.store(true, Ordering::SeqCst);
                let _ = TcpStream::connect_timeout(
                    &SocketAddr::from((Ipv4Addr::LOCALHOST, h.port)),
                    CONNECT_TIMEOUT,
                );
                for s in lock(&h.conns).values() {
                    let _ = s.shutdown(Shutdown::Both);
                }
                if self.engine.close_prefix(&SHARED_MARK.to_string()) {
                    (self.wake)();
                }
            }
            Role::Client(c) => {
                let _ = c.out.shutdown(Shutdown::Both);
            }
            _ => {}
        }
    }

    /// The link broke while sending: find the stack again.
    fn lost_now(&'static self, st: &mut State) {
        let Some(corner) = st.role.corner().map(str::to_string) else {
            return;
        };
        self.leave(st);
        self.rejoin(st, &corner);
    }

    /// The reader of link saw it close (the host quit or gave the corner
    /// up).
    fn lost(&'static self, link: u64) {
        let mut st = lock(&self.st);
        match &st.role {
            Role::Client(c) if c.link == link => {}
            _ => return,
        }
        let corner = st.role.corner().unwrap_or_default().to_string();
        st.role = Role::Idle;
        if self.engine.options().enabled && !st.mine.is_empty() {
            self.rejoin(&mut st, &corner);
        }
    }

    /// Joins corner's stack and puts this program's cards in it again, or
    /// draws them here when this program now hosts or draws alone.
    fn rejoin(&'static self, st: &mut State, corner: &str) {
        st.role = self.elect(st, corner);
        let now = Instant::now();
        st.mine.retain(|m| m.until.is_none_or(|u| u > now));
        if let Role::Client(c) = &st.role {
            let frames: Vec<Msg> = st.mine.iter().filter_map(|m| post_msg(m, now)).collect();
            let out = c.out.try_clone();
            let ok = out.is_ok_and(|o| frames.iter().all(|f| send(&o, f).is_ok()));
            if !ok {
                self.leave(st);
                st.role = Role::Solo {
                    corner: corner.to_string(),
                    since: now,
                };
            } else {
                return;
            }
        }
        for m in std::mem::take(&mut st.mine) {
            let mut n = m.note;
            if let Some(u) = m.until {
                n.expire = u
                    .saturating_duration_since(now)
                    .max(Duration::from_millis(1));
            }
            let _ = self.engine.post(n, now); // no second sound
        }
        (self.wake)();
    }

    /// Hosts corner, or joins its host, or (neither working) draws alone.
    fn elect(&'static self, st: &mut State, corner: &str) -> Role {
        let solo = Role::Solo {
            corner: corner.to_string(),
            since: Instant::now(),
        };
        let Some(dir) = &self.dir else { return solo };
        if fs::create_dir_all(dir).is_err() {
            return solo;
        }
        let lock_path = dir.join(format!("{corner}.lock"));
        let addr_path = dir.join(format!("{corner}.addr"));
        for _ in 0..TRIES {
            let Ok(f) = OpenOptions::new()
                .create(true)
                .truncate(false)
                .write(true)
                .open(&lock_path)
            else {
                return solo;
            };
            match f.try_lock() {
                Ok(()) => {
                    return self.host(f, corner, &addr_path).unwrap_or(solo);
                }
                Err(TryLockError::WouldBlock) => {
                    drop(f);
                    match self.join(&addr_path, corner, st.link + 1) {
                        Ok(c) => {
                            st.link += 1;
                            return Role::Client(c);
                        }
                        Err(true) => std::thread::sleep(TRY_GAP),
                        Err(false) => return solo,
                    }
                }
                Err(TryLockError::Error(_)) => return solo,
            }
        }
        solo
    }

    fn host(&'static self, lock_file: File, corner: &str, addr_path: &Path) -> Option<Role> {
        let l = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).ok()?;
        let port = l.local_addr().ok()?.port();
        let token = token();
        let tmp = addr_path.with_extension(format!("addr.{}", std::process::id()));
        fs::write(&tmp, format!("{port} {token}\n")).ok()?;
        let mut moved = false;
        for _ in 0..TRIES {
            if fs::rename(&tmp, addr_path).is_ok() {
                moved = true;
                break;
            }
            std::thread::sleep(TRY_GAP);
        }
        if !moved {
            let _ = fs::remove_file(&tmp);
            return None;
        }
        let stop = Arc::new(AtomicBool::new(false));
        let conns = Arc::new(Mutex::new(HashMap::new()));
        let (s2, c2) = (stop.clone(), conns.clone());
        std::thread::Builder::new()
            .name("toast-share".into())
            .spawn(move || self.accept(l, token, s2, c2))
            .ok()?;
        Some(Role::Host(Host {
            corner: corner.to_string(),
            _lock: lock_file,
            port,
            stop,
            conns,
        }))
    }

    fn accept(
        &'static self,
        l: TcpListener,
        token: String,
        stop: Arc<AtomicBool>,
        conns: Arc<Mutex<HashMap<u64, TcpStream>>>,
    ) {
        let mut next = 0u64;
        loop {
            let got = l.accept();
            if stop.load(Ordering::SeqCst) {
                return;
            }
            match got {
                Ok((s, _)) => {
                    next += 1;
                    let (id, token, stop, conns) =
                        (next, token.clone(), stop.clone(), conns.clone());
                    let _ = std::thread::Builder::new()
                        .name("toast-share-link".into())
                        .spawn(move || self.serve(s, id, &token, &stop, &conns));
                }
                Err(_) => std::thread::sleep(Duration::from_millis(40)),
            }
        }
    }

    /// One client's link, on the host.
    fn serve(
        &'static self,
        mut s: TcpStream,
        id: u64,
        token: &str,
        stop: &AtomicBool,
        conns: &Mutex<HashMap<u64, TcpStream>>,
    ) {
        let _ = s.set_nonblocking(false);
        let _ = s.set_nodelay(true);
        let _ = s.set_read_timeout(Some(LINK_TIMEOUT));
        let _ = s.set_write_timeout(Some(LINK_TIMEOUT));
        let (pid, look) = match read_msg(&mut s) {
            Ok(Msg::Hello {
                wire,
                token: t,
                pid,
                look,
            }) if wire == WIRE && t == token => (pid, look),
            _ => return,
        };
        let _ = s.set_read_timeout(None);
        let Ok(out) = s.try_clone() else { return };
        let out = Arc::new(Mutex::new(out));
        {
            let mut c = lock(conns);
            if stop.load(Ordering::SeqCst) {
                return;
            }
            if send(&lock(&out), &Msg::Welcome).is_err() {
                return;
            }
            if let Ok(k) = s.try_clone() {
                c.insert(id, k);
            }
        }
        let prefix = format!("{SHARED_MARK}{id}{SHARED_MARK}");
        let mut style = Arc::new(*look);
        while let Ok(m) = read_msg(&mut s) {
            let sync = match m {
                Msg::Post(p) => {
                    let burst = p.burst;
                    let n = remote_note(*p, &prefix, &out, pid);
                    self.engine
                        .post_shared(n, style.clone(), burst, Instant::now())
                }
                Msg::Close(cid) => self.engine.close(&format!("{prefix}{cid}")),
                Msg::DismissAll => self.engine.dismiss_prefix(&prefix),
                Msg::Restyle(c) => {
                    style = Arc::new(*c);
                    self.engine.restyle_prefix(&prefix, style.clone())
                }
                _ => false,
            };
            if sync {
                (self.wake)();
            }
        }
        lock(conns).remove(&id);
        if self.engine.close_prefix(&prefix) {
            (self.wake)();
        }
    }

    /// Connects to corner's host. `Err(true)`: worth another try (the host
    /// has not written its address yet, or an old one is there); `Err(false)`:
    /// the host took the link but did not answer, so draw alone.
    fn join(&'static self, addr_path: &Path, corner: &str, link: u64) -> Result<Client, bool> {
        let (addr, token) = (|| {
            let a = fs::read_to_string(addr_path).ok()?;
            let mut parts = a.split_whitespace();
            let port: u16 = parts.next()?.parse().ok()?;
            let token = parts.next()?.to_string();
            let s = TcpStream::connect_timeout(
                &SocketAddr::from((Ipv4Addr::LOCALHOST, port)),
                CONNECT_TIMEOUT,
            )
            .ok()?;
            Some((s, token))
        })()
        .ok_or(true)?;
        self.hello(addr, token, corner, link).ok_or(false)
    }

    fn hello(
        &'static self,
        mut s: TcpStream,
        token: String,
        corner: &str,
        link: u64,
    ) -> Option<Client> {
        let _ = s.set_nodelay(true);
        s.set_read_timeout(Some(LINK_TIMEOUT)).ok()?;
        s.set_write_timeout(Some(LINK_TIMEOUT)).ok()?;
        let hello = Msg::Hello {
            wire: WIRE,
            token,
            pid: std::process::id(),
            look: Box::new(self.engine.config()),
        };
        send(&s, &hello).ok()?;
        if read_msg(&mut s).ok()? != Msg::Welcome {
            return None;
        }
        s.set_read_timeout(None).ok()?;
        let r = s.try_clone().ok()?;
        std::thread::Builder::new()
            .name("toast-share-link".into())
            .spawn(move || self.read_events(r, link))
            .ok()?;
        Some(Client {
            corner: corner.to_string(),
            out: s,
            link,
        })
    }

    /// A client's link: the host's reports on its cards.
    fn read_events(&'static self, mut s: TcpStream, link: u64) {
        while let Ok(m) = read_msg(&mut s) {
            let (serial, what) = match m {
                Msg::Action(s, i) => (s, Some(Some(i as usize))),
                Msg::Click(s) => (s, Some(None)),
                Msg::Dismiss(s) => (s, None),
                Msg::Gone(s) => {
                    let gone = {
                        let mut st = lock(&self.st);
                        st.mine
                            .iter()
                            .position(|m| m.serial == s)
                            .map(|i| st.mine.remove(i))
                    };
                    drop(gone);
                    continue;
                }
                _ => continue,
            };
            let m = {
                let mut st = lock(&self.st);
                st.mine
                    .iter()
                    .position(|m| m.serial == serial)
                    .map(|i| st.mine.remove(i))
            };
            let Some(m) = m else { continue };
            let n = m.note;
            let f = match what {
                Some(Some(i)) => n.actions.get(i).and_then(|a| a.run.clone()),
                Some(None) => n.on_click.clone(),
                None => n.on_dismiss.clone(),
            };
            run_callback(f, &self.engine.config().log_prefix());
        }
        self.lost(link);
    }
}

/// The post for m with the time it has left; `None` once it has none.
fn post_msg(m: &Mine, now: Instant) -> Option<Msg> {
    let n = &m.note;
    let expire_ms = match m.until {
        None => 0,
        Some(u) => {
            let left = u.saturating_duration_since(now);
            if left.is_zero() {
                return None;
            }
            (left.as_millis() as u64).max(1)
        }
    };
    Some(Msg::Post(Box::new(PostMsg {
        serial: m.serial,
        id: m.id.clone(),
        title: n.title.clone(),
        body: n.body.clone(),
        kind: n.kind,
        sticky: n.sticky,
        expire_ms,
        burst: m.burst,
        clickable: n.on_click.is_some(),
        actions: n
            .actions
            .iter()
            .map(|a| (a.label.clone(), a.run.is_some()))
            .collect(),
        // A picture too big for a frame is left out, not the card.
        image: n
            .image
            .as_ref()
            .filter(|i| i.rgba.len() <= MAX_IMAGE)
            .map(|i| (i.w, i.h, i.rgba.to_vec())),
    })))
}

/// A client's card as a note on the host: its callbacks report back on out.
fn remote_note(p: PostMsg, prefix: &str, out: &Arc<Mutex<TcpStream>>, pid: u32) -> Note {
    let r = Arc::new(Report {
        out: out.clone(),
        serial: p.serial,
        pid,
    });
    let cb = |m: Msg| -> Callback {
        let r = r.clone();
        Arc::new(move || r.send(m.clone()))
    };
    Note {
        id: format!("{prefix}{}", p.id),
        title: p.title,
        body: p.body,
        image: p.image.and_then(|(w, h, rgba)| Image::new(w, h, rgba)),
        kind: p.kind,
        silent: true,
        expire: Duration::from_millis(p.expire_ms),
        sticky: p.sticky,
        actions: p
            .actions
            .into_iter()
            .enumerate()
            .map(|(i, (label, run))| Action {
                label,
                run: run.then(|| cb(Msg::Action(p.serial, i as u8))),
            })
            .collect(),
        on_click: p.clickable.then(|| cb(Msg::Click(p.serial))),
        on_dismiss: Some(cb(Msg::Dismiss(p.serial))),
    }
}
