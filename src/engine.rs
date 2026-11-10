//! The card engine shared by both platforms: the model under one lock and
//! the window-side state that syncs windows to it (go-toast's `syncCards`,
//! `measure`, the mouse handlers and `click`, which Go writes once per OS),
//! plus the stack's motion: where each card goes ([`crate::layout::arrange`]),
//! hover, the wheel, drags and every animation.
//!
//! A platform implements [`Surface`] (create, draw, place, fade, restack and
//! destroy one window per card, measure text, find the work area and the
//! cursor, run a tick and a frame timer) and calls [`Engine::sync`],
//! [`Engine::tick`], [`Engine::frame`], [`Engine::rehome`],
//! [`Engine::mouse`] and [`Engine::wheel`] on its window thread. Tests
//! drive the same engine with a recording surface, headless.
//!
//! Rules kept from Go: the window thread never runs a note's callbacks
//! itself (each gets its own thread once the card is out of the model and
//! the lock is free, so a callback that posts, closes or blocks can't wedge
//! the windows); a click only takes the card out of the model and asks for
//! a sync, so the clicked window isn't destroyed inside its own message.
//!
//! Motion ([`crate::Motion`]): every card has a timeline from where it was
//! to where the layout wants it, eased; the frame timer runs while any
//! timeline does (and while the stack is open, to see the cursor leave
//! it), none at rest. A card that leaves the model keeps its window as an
//! input-dead ghost until its exit plays out. With the OS's reduce-motion
//! setting on, every timeline jumps to its end.

use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crate::config::{Config, Ease, Kind, Motion, Palette};
use crate::layout::{
    CARD_GAP_DIP, CARD_MARGIN_DIP, CARD_WIDTH_DIP, MAX_BODY_LINES, Part, Pose, Rect, StackSpec,
    TEXT_LEFT_DIP, TEXT_RIGHT_DIP, arrange, bounds, button_rects, card_height_dip, close_rect,
    ease, hit_part, is_left, is_top, px, thumb_rect, thumb_size_dip,
};
use crate::note::{Callback, FULL_ALPHA, Image, Note, Options, Stack, clean};
use crate::sound::{Sound, sound_for};

/// What a window shows: everything a platform needs to draw a card.
#[derive(Debug, Clone, PartialEq)]
pub struct Look {
    pub key: u64,
    pub title: String,
    pub body: String,
    pub kind: Kind,
    pub labels: Vec<String>,
    /// Body lines (0, 1 or 2).
    pub lines: usize,
    /// The thumbnail and where it goes (px), under the body.
    pub image: Option<(Image, Rect)>,
    /// The window's size now (px): smaller than the card's own while it is
    /// scaled down in a deck or on its way in or out.
    pub w: i32,
    pub h: i32,
    /// The pixel scale (1 on macOS, where geometry is in points), times
    /// the card's own scale while it is scaled down: draw text and pieces
    /// at it and the card stays crisp at any size.
    pub scale: f64,
    /// Corner radius in px.
    pub radius: i32,
    pub close: Rect,
    pub buttons: Vec<Rect>,
    /// The palette in effect (light or dark, per the config's scheme).
    pub palette: Palette,
    /// The card is dark (the platform's material and appearance follow).
    pub dark: bool,
    /// Real glass: the palette's tint at this opacity over the blurred
    /// desktop. `None` is a solid card (and the fallback without blur).
    pub glass: Option<f64>,
    /// Draw the kind-coloured stripe on the left ([`crate::Config::stripe`]).
    pub stripe: bool,
    /// The part under the cursor.
    pub hot: Part,
    /// The cursor is over this card.
    pub hovered: bool,
    /// The body has a click action (a hand cursor).
    pub clickable: bool,
    /// Folded behind a deck's front card: draw only the card's body (its
    /// background and border), no stripe, icon, text or buttons.
    pub folded: bool,
}

impl Look {
    /// The cursor a platform shows over the card.
    pub fn hand_cursor(&self) -> bool {
        !self.folded
            && (matches!(self.hot, Part::Button(_) | Part::Close)
                || (self.hot == Part::Body && self.clickable))
    }
}

/// A platform's windows, driven on its window thread.
pub trait Surface {
    /// The work area (px, top-down) and scale of the screen a new stack
    /// goes on (`active`: where the person is looking) or of the one the
    /// stack at `old` is on (the active one when it is gone).
    fn work_area(&mut self, active: bool, old: Rect) -> (Rect, f64);
    /// How many lines `body` wraps to in `width` px (1 or 2 once clamped).
    fn body_lines(&mut self, body: &str, width: i32, scale: f64) -> usize;
    /// A button label's text width in px; `None` when it can't measure.
    fn label_width(&mut self, label: &str, scale: f64) -> Option<i32>;
    /// Makes the card's window, hidden and fully transparent.
    fn create(&mut self, key: u64, cfg: &Config, w: i32, h: i32) -> bool;
    /// The card's content, hover state or size changed: `look.w` x
    /// `look.h` is the size the next [`Surface::place`] gives the window.
    fn draw(&mut self, look: &Look);
    /// Moves and sizes the window (px, top-down) without changing its
    /// stacking order. Called every frame while the card moves.
    fn place(&mut self, key: u64, x: i32, y: i32, w: i32, h: i32);
    /// Puts these shown windows back on top of the topmost band, in this
    /// order: the last ends up in front.
    fn restack(&mut self, keys: &[u64]);
    fn alpha(&mut self, key: u64, a: u8);
    /// Shows it without activating anything.
    fn show(&mut self, key: u64);
    fn destroy(&mut self, key: u64);
    /// Runs or stops the expiry tick ([`TICK_MS`]): [`Engine::tick`], then
    /// [`Engine::sync`].
    fn timer(&mut self, on: bool);
    /// Runs or stops the frame timer ([`FRAME_MS`]): [`Engine::frame`]. On
    /// while anything moves and while the stack is open; off at rest.
    fn frames(&mut self, on: bool);
    /// Where the cursor is (px, top-down, the work area's space); `None`
    /// when it can't tell.
    fn cursor(&mut self) -> Option<(i32, i32)>;
    /// The OS asks for reduced motion: every animation jumps to its end.
    fn reduce_motion(&mut self) -> bool;
    /// The stack opened (true) under the cursor or folded again (false).
    /// The Win32 side hooks the wheel meanwhile when the OS sends it to
    /// the focused window rather than the one under the cursor.
    fn hovered(&mut self, _open: bool) {}
    /// Throws a confetti burst from a success card at `card` across the
    /// screen whose work area is `work` (px, top-down), over everything and
    /// without taking clicks. Called once per such post, after the card is
    /// placed. The platform skips it while the OS asks for reduced motion.
    fn confetti(&mut self, _card: Rect, _work: Rect) {}
    /// Whether the OS is in dark mode now (asked only while the config's
    /// scheme is [`crate::config::Scheme::System`]). The platform syncs
    /// again when it flips.
    fn os_dark(&mut self) -> bool {
        true
    }
}

/// How the next sync places the stack, in increasing strength.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Reloc {
    None,
    /// Re-place the stack on its screen (it may have changed).
    Rehome,
    /// A new stack: the screen the person is on.
    Active,
}

/// The tick interval while any card counts down.
pub const TICK_MS: u32 = 30;
/// The frame interval while anything moves.
pub const FRAME_MS: u32 = 16;

/// A mouse event on a card, in its top-down pixel coordinates (relative to
/// the window as it is placed now). While the left button is down the
/// platform captures the mouse, so moves keep coming off the card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mouse {
    Move,
    Leave,
    Down,
    Up,
    /// A right click (or a Control-click on the Mac) dismisses.
    RightUp,
    /// The press was cut off (the capture was taken away): no click, the
    /// card goes back to its place.
    Cancel,
}

/// One step of the mouse wheel over the stack: away from the person
/// (`Up`, scrolling up) or toward them. Each platform maps its own sign.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wheel {
    Up,
    Down,
}

struct Shared {
    stack: Stack,
    reloc: Reloc,
    raise: bool,
    cfg: Config,
    /// Bumped by every configure: cards made under an older one restyle.
    cfg_rev: u64,
    opts: Options,
    /// Card keys on screen, oldest first (tests).
    keys: Vec<u64>,
    /// Success cards posted with confetti on, burst at the next sync once
    /// they are placed.
    confetti: Vec<u64>,
}

const LINEAR: Ease = [0.0, 0.0, 1.0, 1.0];

/// A card's way from one pose to another.
#[derive(Debug, Clone, Copy)]
struct Anim {
    from: Pose,
    to: Pose,
    start: Instant,
    /// Position and size take dur on `ease`, opacity fade on `fade_ease`.
    dur: Duration,
    fade: Duration,
    ease: Ease,
    fade_ease: Ease,
}

impl Anim {
    /// Standing at p.
    fn still(p: Pose, now: Instant) -> Anim {
        Anim {
            from: p,
            to: p,
            start: now,
            dur: Duration::ZERO,
            fade: Duration::ZERO,
            ease: LINEAR,
            fade_ease: LINEAR,
        }
    }

    /// An ordinary move: the stack's ease, opacity a little quicker.
    fn moving(from: Pose, to: Pose, now: Instant, m: &Motion) -> Anim {
        Anim {
            from,
            to,
            start: now,
            dur: m.duration,
            fade: m.duration.mul_f64(0.7),
            ease: m.ease,
            fade_ease: m.ease_out,
        }
    }

    fn at(&self, now: Instant) -> Pose {
        let gone = now.saturating_duration_since(self.start).as_secs_f64();
        let k = |d: Duration, e: Ease| {
            if d.is_zero() {
                1.0
            } else {
                ease(e, gone / d.as_secs_f64())
            }
        };
        let (m, f) = (
            k(self.dur, self.ease),
            k(self.fade, self.fade_ease).clamp(0.0, 1.0),
        );
        let (a, b) = (self.from, self.to);
        let lerp = |x: f64, y: f64| x + (y - x) * m;
        Pose {
            x: lerp(a.x, b.x),
            y: lerp(a.y, b.y),
            w: lerp(a.w, b.w).max(1.0),
            h: lerp(a.h, b.h).max(1.0),
            o: (a.o + (b.o - a.o) * f).clamp(0.0, 1.0),
        }
    }

    fn done(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.start) >= self.dur.max(self.fade)
    }
}

/// One card's window and its copy of the content.
struct CardWin {
    key: u64,
    rev: u64,
    scale: f64,
    cfg: Config,
    /// The configure revision and darkness cfg was drawn with.
    cfg_rev: u64,
    dark: bool,
    title: String,
    body: String,
    kind: Kind,
    labels: Vec<String>,
    clickable: bool,
    image: Option<Image>,
    thumb: Rect,
    lines: usize,
    /// The card's own size, px.
    w: i32,
    h: i32,
    radius: i32,
    close: Rect,
    buttons: Vec<Rect>,
    hot: Part,
    pressed: Part,
    /// The hover state last drawn.
    hovered: bool,

    /// Where the layout puts it, folded or not, and its stacking order.
    slot: Pose,
    folded: bool,
    z: i32,
    anim: Anim,
    /// New: the next layout slides it in from the edge.
    fresh: bool,
    /// Under a drag: the drag places it, not the layout.
    held: bool,
    /// Out of the stack, playing its exit; destroyed once that is over and
    /// the model has let it go.
    ghost: bool,
    in_model: bool,
    /// On screen now: the window's rect and alpha, the (w, h, folded) it
    /// was drawn at, and whether it was ever placed.
    rect: Rect,
    alpha: u8,
    drawn: Option<(i32, i32, bool)>,
    placed: bool,
}

/// Which way a drag went, decided once in its first [`Motion::lock`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Axis {
    Deciding,
    /// Sideways: the card follows and is thrown away or springs back.
    Dismiss,
    /// Up or down while the open stack has hidden cards: moves the window.
    Scroll,
    /// Up or down with nothing to scroll: a little give, then back.
    Rubber,
}

#[derive(Debug, Clone, Copy)]
struct Drag {
    key: u64,
    /// Where it started and was last, on screen.
    sx: i32,
    sy: i32,
    lx: i32,
    ly: i32,
    lt: Instant,
    /// Sideways speed at the last move, px/ms.
    v: f64,
    axis: Axis,
    /// Scroll steps taken.
    stepped: i32,
}

struct Ui {
    wins: BTreeMap<u64, CardWin>,
    work: Rect,
    scale: f64,
    /// The card under the cursor, 0 = none.
    hover_key: u64,
    ticking: bool,
    corner: String,
    motion: Motion,
    reduce: bool,
    /// The live cards (not ghosts), oldest first.
    order: Vec<u64>,
    /// The stack is open (hovered).
    open: bool,
    /// Open with too many to fit: the first card shown in full and the
    /// largest that can be.
    at: Option<usize>,
    max_at: usize,
    /// Around every live card's slot: the cursor in it holds the stack open.
    bounds: Rect,
    /// A leave just after a throw folds the stack only at this time.
    fold_at: Option<Instant>,
    throw_hold: Option<Instant>,
    wheel_at: Option<Instant>,
    drag: Option<Drag>,
    /// The order last given to restack, and a restack owed regardless.
    z: Vec<u64>,
    restack: bool,
    framing: bool,
    told_open: bool,
}

/// What a post did.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Posted {
    /// The card is in the model (the window thread should sync).
    pub shown: bool,
    /// The sound to play, at this volume.
    pub sound: Option<(Sound, f64)>,
}

/// The cards' model and window state.
pub struct Engine {
    shared: Mutex<Shared>,
    ui: Mutex<Ui>,
}

impl Default for Engine {
    fn default() -> Self {
        Engine::new()
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Runs a callback on its own thread; a panic is logged, not spread.
pub fn run_callback(f: Option<Callback>, log_prefix: &str) {
    let Some(f) = f else { return };
    let prefix = log_prefix.to_string();
    let spawned = std::thread::Builder::new()
        .name("toast-callback".into())
        .spawn(move || {
            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f())).is_err() {
                eprintln!("{prefix}notification callback panicked");
            }
        });
    if let Err(e) = spawned {
        eprintln!("{log_prefix}can't run a notification callback: {e}");
    }
}

impl Engine {
    pub fn new() -> Engine {
        let cfg = Config::default().normalized();
        let mut stack = Stack::default();
        stack.max = cfg.motion.max_cards;
        Engine {
            shared: Mutex::new(Shared {
                stack,
                reloc: Reloc::None,
                raise: false,
                cfg: cfg.clone(),
                cfg_rev: 0,
                opts: Options::default(),
                keys: vec![],
                confetti: vec![],
            }),
            ui: Mutex::new(Ui {
                wins: BTreeMap::new(),
                work: Rect::default(),
                scale: 1.0,
                hover_key: 0,
                ticking: false,
                corner: Options::default().corner,
                motion: cfg.motion,
                reduce: false,
                order: vec![],
                open: false,
                at: None,
                max_at: 0,
                bounds: Rect::default(),
                fold_at: None,
                throw_hold: None,
                wheel_at: None,
                drag: None,
                z: vec![],
                restack: false,
                framing: false,
                told_open: false,
            }),
        }
    }

    /// Sets the app-level setup. Cards already up restyle at the next
    /// sync (their window class and title stay). True when it changed.
    pub fn configure(&self, c: Config) -> bool {
        let c = c.normalized();
        let mut s = lock(&self.shared);
        if s.cfg == c {
            return false;
        }
        s.stack.max = c.motion.max_cards;
        s.cfg = c;
        s.cfg_rev += 1;
        true
    }

    /// The setup in effect, defaults filled in.
    pub fn config(&self) -> Config {
        lock(&self.shared).cfg.clone()
    }

    /// Applies the person's settings; turning cards off clears them.
    /// Returns true: the window thread should sync (a corner change moves
    /// the stack).
    pub fn set_options(&self, o: Options) -> bool {
        let o = o.normalized();
        let mut s = lock(&self.shared);
        if !o.enabled {
            s.stack.cards.clear();
        }
        s.opts = o;
        true
    }

    pub fn options(&self) -> Options {
        lock(&self.shared).opts.clone()
    }

    /// Puts n in the stack (or replaces the card with its id). A no-op when
    /// cards are off or the note has nothing to say.
    pub fn post(&self, n: Note, now: Instant) -> Posted {
        let mut s = lock(&self.shared);
        let o = s.opts.clone();
        let none = Posted {
            shown: false,
            sound: None,
        };
        if !o.enabled {
            return none;
        }
        let Some(n) = clean(n) else { return none };
        if s.stack.cards.is_empty() {
            s.reloc = Reloc::Active; // a new stack goes where the person is looking
        } else if s.reloc < Reloc::Rehome {
            s.reloc = Reloc::Rehome; // a screen unplugged under a sticky card moves the stack
        }
        let sound =
            (!n.silent && o.sound).then(|| (sound_for(n.kind, !n.actions.is_empty()), o.volume));
        let burst = o.confetti && n.kind == Kind::Success;
        let (key, _) = s.stack.post(n, now);
        if burst && !s.confetti.contains(&key) {
            s.confetti.push(key);
        }
        s.raise = true;
        Posted { shown: true, sound }
    }

    /// Removes the card with id without running its callbacks; true when one
    /// was up (sync).
    pub fn close(&self, id: &str) -> bool {
        lock(&self.shared).stack.close_id(id)
    }

    /// A display or work-area change: re-place the stack on the next sync.
    pub fn rehome(&self) {
        let mut s = lock(&self.shared);
        if !s.stack.cards.is_empty() && s.reloc < Reloc::Rehome {
            s.reloc = Reloc::Rehome;
            s.raise = true;
        }
    }

    /// Advances expiry (the tick timer), then the caller syncs. Nothing
    /// expires while the stack is open.
    pub fn tick(&self, now: Instant) {
        let hovered = lock(&self.ui).open;
        lock(&self.shared).stack.tick(now, hovered);
    }

    /// The cards on screen, oldest first (not those on their way out).
    pub fn keys(&self) -> Vec<u64> {
        lock(&self.shared).keys.clone()
    }

    /// Whether any card window is up (one on its way out counts).
    pub fn visible(&self) -> bool {
        !lock(&self.ui).wins.is_empty()
    }

    /// Whether the stack is open (hovered, or held open after a throw).
    pub fn is_open(&self) -> bool {
        lock(&self.ui).open
    }

    /// Open with more cards than fit: the index (oldest first) of the
    /// first card shown in full, and the largest it can be. `None` when
    /// every card is laid out.
    pub fn scroll(&self) -> Option<(usize, usize)> {
        let ui = lock(&self.ui);
        ui.at.map(|a| (a, ui.max_at))
    }

    /// Where the layout puts card key now and whether it is folded behind
    /// a deck's front (tests): the target, not the moving window.
    pub fn slot(&self, key: u64) -> Option<(Rect, bool)> {
        lock(&self.ui)
            .wins
            .get(&key)
            .filter(|c| !c.ghost)
            .map(|c| (c.slot.rect(), c.folded))
    }

    /// Whether screen point (x, y) is on the open stack. It never waits
    /// for the lock (a low-level hook asks), and says no when it can't
    /// tell.
    pub fn over_stack(&self, x: i32, y: i32) -> bool {
        match self.ui.try_lock() {
            Ok(ui) => ui.open && ui.bounds.has(x, y),
            Err(_) => false,
        }
    }

    /// The model's notes, oldest first (tests).
    pub fn notes(&self) -> Vec<Note> {
        lock(&self.shared)
            .stack
            .cards
            .iter()
            .map(|c| c.note.clone())
            .collect()
    }

    /// Makes the windows match the model: creates and re-measures them,
    /// lays the stack out and starts each card toward its slot, starts the
    /// exit of cards that left (or are expiring), and runs the tick timer
    /// while any card counts down (a stack of only sticky cards needs
    /// none).
    pub fn sync(&self, surf: &mut dyn Surface, now: Instant) {
        struct View {
            key: u64,
            rev: u64,
            title: String,
            body: String,
            image: Option<Image>,
            kind: Kind,
            labels: Vec<String>,
            clickable: bool,
            closing: bool,
        }
        let mut timed = false;
        let (views, reloc, up, corner, cfg, cfg_rev, bursts) = {
            let mut s = lock(&self.shared);
            let views: Vec<View> = s
                .stack
                .cards
                .iter()
                .map(|c| {
                    if c.until.is_some() || c.closing.is_some() {
                        timed = true;
                    }
                    View {
                        key: c.key,
                        rev: c.rev,
                        title: c.note.title.clone(),
                        body: c.note.body.clone(),
                        image: c.note.image.clone(),
                        kind: c.note.kind,
                        labels: c.note.actions.iter().map(|a| a.label.clone()).collect(),
                        clickable: c.note.on_click.is_some(),
                        closing: c.closing.is_some(),
                    }
                })
                .collect();
            let out = (
                views,
                s.reloc,
                s.raise,
                s.opts.corner.clone(),
                s.cfg.clone(),
                s.cfg_rev,
                std::mem::take(&mut s.confetti),
            );
            s.reloc = Reloc::None;
            s.raise = false;
            out
        };

        let mut guard = lock(&self.ui);
        let ui = &mut *guard;
        if reloc != Reloc::None && !views.is_empty() {
            let (w, sc) = surf.work_area(reloc == Reloc::Active, ui.work);
            ui.work = w;
            ui.scale = sc;
        }
        let sc = ui.scale;
        ui.reduce = surf.reduce_motion();
        ui.corner = corner;
        ui.motion = cfg.motion.clone();
        let os_dark = cfg.scheme == crate::config::Scheme::System && surf.os_dark();
        let dark = cfg.is_dark(os_dark);

        for cw in ui.wins.values_mut() {
            cw.in_model = false;
        }
        let hover_key = ui.hover_key;
        let mut order = vec![];
        for v in views {
            let fresh = !ui.wins.contains_key(&v.key);
            let mut cw = ui
                .wins
                .remove(&v.key)
                .unwrap_or_else(|| new_win(v.key, &cfg, cfg_rev, dark, now));
            let restyle = cw.cfg_rev != cfg_rev || cw.dark != dark;
            if restyle {
                cw.cfg = cfg.clone();
                cw.cfg_rev = cfg_rev;
                cw.dark = dark;
            }
            if fresh || restyle || cw.rev != v.rev || cw.scale != sc {
                cw.rev = v.rev;
                cw.scale = sc;
                cw.title = v.title;
                cw.body = v.body;
                cw.kind = v.kind;
                cw.labels = v.labels;
                cw.clickable = v.clickable;
                cw.image = v.image;
                measure(&mut cw, surf);
                if fresh && !surf.create(cw.key, &cw.cfg, cw.w, cw.h) {
                    continue;
                }
                cw.hovered = hover_key == cw.key;
                cw.drawn = None; // drawn when it is next placed
            }
            cw.in_model = true;
            if v.closing {
                // Expiring: it plays its exit while the model fades it.
                if !cw.ghost {
                    exit(&mut cw, None, &ui.corner, sc, &ui.motion, now);
                }
            } else {
                // Back from a fade (the cursor came back) or never left.
                cw.ghost = false;
                order.push(cw.key);
            }
            ui.wins.insert(cw.key, cw);
        }
        // Cards that left the model (clicked, closed, replaced past the
        // limit) play their exit, then go.
        let corner = ui.corner.clone();
        let motion = ui.motion.clone();
        for cw in ui.wins.values_mut() {
            if !cw.in_model && !cw.ghost {
                exit(cw, None, &corner, sc, &motion, now);
            }
        }
        if let Some(cw) = ui.wins.get(&ui.hover_key)
            && cw.ghost
        {
            ui.hover_key = 0; // input-dead
        }
        if let Some(d) = ui.drag
            && ui.wins.get(&d.key).is_none_or(|c| c.ghost)
        {
            ui.drag = None;
        }
        ui.order = order;
        if ui.order.is_empty() {
            ui.open = false;
            ui.at = None;
            ui.fold_at = None;
            ui.throw_hold = None;
        }
        ui.relayout(now);
        if up {
            ui.restack = true;
        }
        ui.apply(surf, now);

        for k in bursts {
            if let Some(cw) = ui.wins.get(&k).filter(|cw| cw.placed && !cw.ghost) {
                surf.confetti(cw.slot.rect(), ui.work);
            }
        }

        let keys = ui.order.clone();
        if timed && !ui.ticking {
            surf.timer(true);
            ui.ticking = true;
        } else if !timed && ui.ticking {
            surf.timer(false);
            ui.ticking = false;
        }
        drop(guard);
        lock(&self.shared).keys = keys;
    }

    /// A frame (the frame timer): advances every card's timeline, ends the
    /// exits that are over, and folds the stack once the cursor has left it
    /// (or the hold after a throw ran out).
    pub fn frame(&self, surf: &mut dyn Surface, now: Instant) {
        let mut guard = lock(&self.ui);
        let ui = &mut *guard;
        if ui.open
            && ui.drag.is_none()
            && let Some((x, y)) = surf.cursor()
        {
            if ui.bounds.has(x, y) {
                // Back on the stack (a gap between cards counts) before the
                // hold after a throw ran out: it stays open.
                ui.fold_at = None;
            } else if ui.fold_at.is_none() {
                ui.unhover(surf);
                ui.leave(now);
            }
        }
        if let Some(t) = ui.fold_at
            && now >= t
        {
            ui.fold(now);
        }
        ui.apply(surf, now);
    }

    /// A mouse event on card key at (x, y) in its pixel coordinates. True
    /// when a click or a throw took the card out of the model: the caller
    /// queues a sync (never runs it inside the window's own message).
    pub fn mouse(
        &self,
        surf: &mut dyn Surface,
        key: u64,
        ev: Mouse,
        x: i32,
        y: i32,
        now: Instant,
    ) -> bool {
        let mut guard = lock(&self.ui);
        let ui = &mut *guard;
        let Some(cw) = ui.wins.get(&key) else {
            return false;
        };
        if cw.ghost {
            if ui.hover_key == key && ev == Mouse::Leave {
                ui.hover_key = 0;
            }
            return false;
        }
        let (sx, sy) = (cw.rect.l + x, cw.rect.t + y);
        let (folded, w, h) = (cw.folded, cw.w, cw.h);
        let dragging = ui.drag.is_some_and(|d| d.key == key);
        let mut new_hover = ui.hover_key;
        let mut click = None;
        let mut throw = None;
        match ev {
            Mouse::Move if !dragging && !ui.bounds.has(sx, sy) => {
                // A card passing under a still cursor off the stack (one
                // sliding in from the edge, Windows sends it a move): not
                // the person coming onto the stack, so it neither opens
                // nor hovers.
                if ui.hover_key == key {
                    new_hover = 0;
                    let cw = ui.wins.get_mut(&key).expect("checked above");
                    cw.hot = Part::None;
                }
            }
            Mouse::Move => {
                new_hover = key;
                ui.enter(now);
                if dragging {
                    ui.drag_to(sx, sy, now);
                } else {
                    let cw = ui.wins.get_mut(&key).expect("checked above");
                    cw.hot = if folded {
                        Part::None
                    } else {
                        hit_part(x, y, cw.close, &cw.buttons)
                    };
                }
            }
            Mouse::Leave => {
                if dragging {
                    return false; // the drag decides at its release
                }
                if ui.hover_key == key {
                    new_hover = 0;
                }
                let cw = ui.wins.get_mut(&key).expect("checked above");
                cw.pressed = Part::None;
                cw.hot = Part::None;
                // Off this card, but maybe still on the stack (a gap, or
                // the next card before its move arrives): only leaving the
                // stack's bounds folds it. The frames watch the cursor
                // meanwhile.
                if new_hover == 0 {
                    match surf.cursor() {
                        Some((cx, cy)) if ui.bounds.has(cx, cy) => {}
                        _ => ui.leave(now),
                    }
                }
            }
            Mouse::Down => {
                let cw = ui.wins.get_mut(&key).expect("checked above");
                cw.pressed = if folded {
                    Part::None // a sliver of a card nobody can read takes no clicks
                } else {
                    hit_part(x, y, cw.close, &cw.buttons)
                };
                if cw.pressed == Part::Body {
                    ui.drag = Some(Drag {
                        key,
                        sx,
                        sy,
                        lx: sx,
                        ly: sy,
                        lt: now,
                        v: 0.0,
                        axis: Axis::Deciding,
                        stepped: 0,
                    });
                }
            }
            Mouse::Up => {
                let d = ui.drag.filter(|d| d.key == key);
                let mut moved = false;
                if let Some(d) = d {
                    ui.drag = None;
                    moved = d.axis != Axis::Deciding;
                    let dx = (sx - d.sx) as f64;
                    let m = &ui.motion;
                    if d.axis == Axis::Dismiss
                        && (dx.abs() > m.dismiss_distance * ui.scale
                            || d.v.abs() > m.flick_speed * ui.scale)
                    {
                        throw = Some(if dx != 0.0 { dx.signum() } else { d.v.signum() });
                    } else {
                        ui.let_go(key, now);
                    }
                }
                let cw = ui.wins.get_mut(&key).expect("checked above");
                if throw.is_none() && !moved {
                    // Pressed and released on the same part, like a button;
                    // a release off the card (a press dragged away) is no
                    // click.
                    let p = if x >= 0 && y >= 0 && x < w && y < h && !folded {
                        hit_part(x, y, cw.close, &cw.buttons)
                    } else {
                        Part::None
                    };
                    if p != Part::None && p == cw.pressed {
                        click = Some(p);
                    }
                }
                cw.pressed = Part::None;
                if let Some(dir) = throw {
                    // Out of the stack at once: the others close up while it
                    // flies; the model lets it go below, with its dismiss.
                    let (corner, sc, motion) = (ui.corner.clone(), ui.scale, ui.motion.clone());
                    exit(cw, Some(dir), &corner, sc, &motion, now);
                    ui.order.retain(|k| *k != key);
                    if ui.open {
                        ui.throw_hold = Some(now + ui.motion.after_throw);
                    }
                    ui.relayout(now);
                    new_hover = 0;
                }
                if d.is_some() && !ui.bounds.has(sx, sy) {
                    // Released off the stack: that is the leave.
                    new_hover = 0;
                    ui.hover_key = 0;
                    ui.leave(now);
                }
            }
            Mouse::RightUp => {
                if !folded {
                    click = Some(Part::Close);
                }
            }
            Mouse::Cancel => {
                if dragging {
                    ui.drag = None;
                    ui.let_go(key, now);
                }
                let cw = ui.wins.get_mut(&key).expect("checked above");
                cw.pressed = Part::None;
            }
        }
        if let Some(cw) = ui.wins.get_mut(&key)
            && !cw.ghost
        {
            let hovered = new_hover == key;
            let redraw = cw.hovered != hovered || ev == Mouse::Move || ev == Mouse::Leave;
            cw.hovered = hovered;
            if redraw && cw.drawn.is_some() {
                surf.draw(&look(cw));
            }
        }
        ui.hover_key = new_hover;
        ui.apply(surf, now);
        drop(guard);
        if throw.is_some() {
            return self.click(key, Part::Close); // a person's dismiss
        }
        match click {
            Some(p) => self.click(key, p),
            None => false,
        }
    }

    /// One wheel step over the stack: while it is open with cards hidden
    /// in its decks, moves the window one card (newer for `Up` in a bottom
    /// corner, older in a top one). Steps closer together than
    /// [`Motion::wheel_debounce`] count once. True when it moved.
    pub fn wheel(&self, surf: &mut dyn Surface, dir: Wheel, now: Instant) -> bool {
        let mut guard = lock(&self.ui);
        let ui = &mut *guard;
        let Some(at) = ui.at else { return false };
        if !ui.open || ui.max_at == 0 || ui.drag.is_some() {
            return false;
        }
        if let Some(t) = ui.wheel_at
            && now.saturating_duration_since(t) < ui.motion.wheel_debounce
        {
            return false;
        }
        ui.wheel_at = Some(now);
        let up = if dir == Wheel::Up { 1 } else { -1 };
        let step = if is_top(&ui.corner) { -up } else { up };
        let to = (at as i64 + step).clamp(0, ui.max_at as i64) as usize;
        if to == at {
            return false;
        }
        ui.at = Some(to);
        ui.relayout(now);
        ui.apply(surf, now);
        true
    }

    /// The card leaves the model first, then its callback gets its own
    /// thread. True when a card was taken (sync).
    pub fn click(&self, key: u64, p: Part) -> bool {
        let (c, prefix) = {
            let mut s = lock(&self.shared);
            let prefix = s.cfg.log_prefix();
            (s.stack.take(key), prefix)
        };
        let Some(c) = c else { return false };
        let n = c.note;
        let f = match p {
            Part::Button(i) if i < n.actions.len() => n.actions[i].run.clone(),
            Part::Body if n.on_click.is_some() => n.on_click.clone(),
            _ => n.on_dismiss.clone(),
        };
        run_callback(f, &prefix);
        true
    }

    /// The look of card key now, for a platform's paint (tests too).
    pub fn look(&self, key: u64) -> Option<Look> {
        let ui = lock(&self.ui);
        ui.wins.get(&key).map(look)
    }
}

impl Ui {
    /// Lays the live cards out for the stack's state and starts each one
    /// toward its slot (a new one from the screen edge). A card under a
    /// drag keeps its pose.
    fn relayout(&mut self, now: Instant) {
        let sc = self.scale;
        let m = &self.motion;
        let heights: Vec<i32> = self.order.iter().map(|k| self.wins[k].h).collect();
        let spec = StackSpec {
            work: self.work,
            corner: &self.corner,
            w: px(CARD_WIDTH_DIP, sc),
            gap: px(CARD_GAP_DIP, sc),
            margin: px(CARD_MARGIN_DIP, sc),
            shown: m.shown,
            peek: px(m.peek, sc),
            depth: m.peek_depth,
            shrink: m.peek_shrink,
            fade: m.peek_fade,
        };
        let a = arrange(&spec, &heights, self.open, self.at);
        self.at = a.at;
        self.max_at = a.max_at;
        self.bounds = bounds(&a.slots);
        let top = is_top(&self.corner);
        let edge = if is_left(&self.corner) { -1.0 } else { 1.0 };
        for (k, slot) in self.order.iter().zip(&a.slots) {
            let cw = self.wins.get_mut(k).expect("ordered cards have windows");
            cw.slot = slot.pose;
            cw.folded = slot.folded;
            cw.z = slot.z;
            if cw.held {
                continue;
            }
            if cw.fresh {
                cw.fresh = false;
                let from = slot
                    .pose
                    .scaled(m.enter_scale, !top)
                    .shifted(edge * m.enter_offset * sc, 0.0);
                cw.anim = Anim::moving(Pose { o: 0.0, ..from }, slot.pose, now, m);
            } else if cw.anim.to != cw.slot {
                cw.anim = Anim::moving(cw.anim.at(now), cw.slot, now, m);
            }
        }
    }

    /// Puts every window where its timeline is now: draws it again when its
    /// size or folding changed, places it, fades it, shows it the first
    /// time, ends exits that are over, keeps the stacking order, and runs
    /// the frame timer while anything moves or the stack is open.
    fn apply(&mut self, surf: &mut dyn Surface, now: Instant) {
        let mut moving = false;
        let mut dead = vec![];
        for cw in self.wins.values_mut() {
            let done = self.reduce || cw.anim.done(now);
            let p = if self.reduce {
                cw.anim.to
            } else {
                cw.anim.at(now)
            };
            if cw.ghost && done {
                if !cw.in_model {
                    dead.push(cw.key);
                }
                // Still in the model (fading out there): wait for it, gone.
                if cw.placed && cw.alpha != 0 {
                    cw.alpha = 0;
                    surf.alpha(cw.key, 0);
                }
                continue;
            }
            if !done {
                moving = true;
            }
            let r = p.rect();
            let (w, h) = (r.r - r.l, r.b - r.t);
            if cw.drawn != Some((w, h, cw.folded)) {
                cw.drawn = Some((w, h, cw.folded));
                surf.draw(&look(cw));
            }
            if !cw.placed || r != cw.rect {
                surf.place(cw.key, r.l, r.t, w, h);
                cw.rect = r;
            }
            let a = (p.o * FULL_ALPHA as f64).round() as u8;
            if !cw.placed || a != cw.alpha {
                cw.alpha = a;
                surf.alpha(cw.key, a);
            }
            if !cw.placed {
                cw.placed = true;
                surf.show(cw.key);
                self.restack = true;
            }
        }
        for k in dead {
            self.wins.remove(&k);
            if self.hover_key == k {
                self.hover_key = 0; // no leave comes for a destroyed window
            }
            surf.destroy(k);
        }

        // Ghosts in front (a thrown card flies over the rest), then the card
        // under a drag, then the layout's order.
        let mut zs: Vec<(i32, usize, u64)> = self
            .wins
            .values()
            .filter(|c| c.placed)
            .map(|c| {
                let z = if c.ghost {
                    i32::MAX
                } else if c.held {
                    i32::MAX - 1
                } else {
                    c.z
                };
                let i = self.order.iter().position(|k| *k == c.key);
                (z, i.unwrap_or(usize::MAX), c.key)
            })
            .collect();
        zs.sort();
        let keys: Vec<u64> = zs.into_iter().map(|(_, _, k)| k).collect();
        if self.restack || keys != self.z {
            if !keys.is_empty() {
                surf.restack(&keys);
            }
            self.z = keys;
            self.restack = false;
        }

        let framing = moving || self.open || self.fold_at.is_some();
        if framing != self.framing {
            surf.frames(framing);
            self.framing = framing;
        }
        if self.open != self.told_open {
            surf.hovered(self.open);
            self.told_open = self.open;
        }
    }

    /// The cursor came onto the stack: open it (and forget a fold that
    /// was waiting).
    fn enter(&mut self, now: Instant) {
        self.fold_at = None;
        if !self.open {
            self.open = true;
            self.at = None; // opens on the newest
            self.relayout(now);
        }
    }

    /// The cursor left the stack: fold at once, or when the hold after a
    /// throw runs out.
    fn leave(&mut self, now: Instant) {
        match self.throw_hold {
            Some(h) if now < h => self.fold_at = Some(h),
            _ => self.fold(now),
        }
    }

    fn fold(&mut self, now: Instant) {
        self.fold_at = None;
        self.throw_hold = None;
        if self.open {
            self.open = false;
            self.at = None;
            self.relayout(now);
        }
    }

    /// The card under the cursor is no longer (the cursor left without a
    /// leave message): redraw it plain.
    fn unhover(&mut self, surf: &mut dyn Surface) {
        let k = std::mem::replace(&mut self.hover_key, 0);
        if let Some(cw) = self.wins.get_mut(&k)
            && cw.hovered
        {
            cw.hovered = false;
            cw.hot = Part::None;
            if cw.drawn.is_some() {
                surf.draw(&look(cw));
            }
        }
    }

    /// The drag goes back to the layout: the card springs to its slot.
    fn let_go(&mut self, key: u64, now: Instant) {
        if let Some(cw) = self.wins.get_mut(&key)
            && cw.held
        {
            cw.held = false;
            cw.anim = Anim::moving(cw.anim.at(now), cw.slot, now, &self.motion);
        }
    }

    /// The pointer of the drag is at (sx, sy) on screen.
    fn drag_to(&mut self, sx: i32, sy: i32, now: Instant) {
        let Some(mut d) = self.drag else { return };
        if (sx, sy) == (d.lx, d.ly) {
            // Windows sends a move at the same point when a window moves
            // under the cursor (and the dragged card does): that's no
            // move, and must not zero the flick's speed.
            return;
        }
        let sc = self.scale;
        let ms = (now.saturating_duration_since(d.lt).as_secs_f64() * 1000.0).max(1.0);
        d.v = (sx - d.lx) as f64 / ms;
        d.lx = sx;
        d.ly = sy;
        d.lt = now;
        let (dx, dy) = ((sx - d.sx) as f64, (sy - d.sy) as f64);
        if d.axis == Axis::Deciding && dx.abs().max(dy.abs()) > self.motion.lock * sc {
            d.axis = if dx.abs() >= dy.abs() {
                Axis::Dismiss
            } else if self.open && self.at.is_some() && self.max_at > 0 {
                Axis::Scroll
            } else {
                Axis::Rubber
            };
        }
        self.drag = Some(d);
        let key = d.key;
        match d.axis {
            Axis::Deciding | Axis::Dismiss => {
                let m = &self.motion;
                let o = (1.0 - dx.abs() / (m.drag_fade * sc)).max(m.drag_min_opacity);
                let cw = self.wins.get_mut(&key).expect("dragged card has a window");
                cw.held = true;
                let p = cw.slot.shifted(dx, 0.0);
                cw.anim = Anim::still(Pose { o: p.o * o, ..p }, now);
            }
            Axis::Scroll => {
                // The card is let go at once (it springs back to its slot)
                // and the drag moves the window, a card per step.
                self.let_go(key, now);
                let want = (dy / (self.motion.scroll_step * sc)).trunc() as i32;
                if want != d.stepped {
                    let dir = if is_top(&self.corner) { -1 } else { 1 };
                    let at =
                        self.at.unwrap_or(self.max_at) as i64 + ((want - d.stepped) * dir) as i64;
                    self.at = Some(at.clamp(0, self.max_at as i64) as usize);
                    d.stepped = want;
                    self.drag = Some(d);
                    self.relayout(now);
                }
            }
            Axis::Rubber => {
                let band = self.motion.rubber_band;
                let cw = self.wins.get_mut(&key).expect("dragged card has a window");
                cw.held = true;
                cw.anim = Anim::still(cw.slot.shifted(0.0, dy * band), now);
            }
        }
    }
}

/// A card's window state before its first measure.
fn new_win(key: u64, cfg: &Config, cfg_rev: u64, dark: bool, now: Instant) -> CardWin {
    CardWin {
        key,
        rev: 0,
        scale: 0.0,
        cfg: cfg.clone(),
        cfg_rev,
        dark,
        title: String::new(),
        body: String::new(),
        kind: Kind::Info,
        labels: vec![],
        clickable: false,
        image: None,
        thumb: Rect::default(),
        lines: 0,
        w: 0,
        h: 0,
        radius: 0,
        close: Rect::default(),
        buttons: vec![],
        hot: Part::None,
        pressed: Part::None,
        hovered: false,
        slot: Pose::default(),
        folded: false,
        z: 0,
        anim: Anim::still(Pose::default(), now),
        fresh: true,
        held: false,
        ghost: false,
        in_model: true,
        rect: Rect::default(),
        alpha: 0,
        drawn: None,
        placed: false,
    }
}

/// Starts cw's exit from where it is now: a close slides it toward the
/// screen edge, a little smaller, as it fades; a throw (`Some(direction)`,
/// -1 left or 1 right) flies it off that way.
fn exit(cw: &mut CardWin, throw: Option<f64>, corner: &str, sc: f64, m: &Motion, now: Instant) {
    let p = cw.anim.at(now);
    let edge = if is_left(corner) { -1.0 } else { 1.0 };
    let to = match throw {
        Some(dir) => p.shifted(dir * m.throw_offset * sc, 0.0),
        None => p
            .scaled(m.enter_scale, !is_top(corner))
            .shifted(edge * m.leave_offset * sc, 0.0),
    };
    let dur = m.duration.mul_f64(m.exit);
    cw.anim = Anim {
        from: p,
        to: Pose { o: 0.0, ..to },
        start: now,
        dur,
        fade: dur.mul_f64(0.75),
        ease: m.ease_out,
        fade_ease: LINEAR,
    };
    cw.ghost = true;
    cw.held = false;
    cw.fresh = false;
    cw.hot = Part::None;
    cw.pressed = Part::None;
}

fn look(cw: &CardWin) -> Look {
    let (w, h, folded) = cw.drawn.unwrap_or((cw.w, cw.h, false));
    let s = if cw.w > 0 {
        w as f64 / cw.w as f64
    } else {
        1.0
    };
    let sr = |r: Rect| {
        if w == cw.w {
            return r;
        }
        let f = |v: i32| (v as f64 * s).round() as i32;
        Rect {
            l: f(r.l),
            t: f(r.t),
            r: f(r.r),
            b: f(r.b),
        }
    };
    Look {
        key: cw.key,
        title: cw.title.clone(),
        body: cw.body.clone(),
        kind: cw.kind,
        labels: cw.labels.clone(),
        lines: cw.lines,
        image: cw.image.clone().map(|i| (i, sr(cw.thumb))),
        w,
        h,
        scale: cw.scale * s,
        radius: cw.radius,
        close: sr(cw.close),
        buttons: cw.buttons.iter().map(|b| sr(*b)).collect(),
        palette: cw.cfg.palette(cw.dark),
        dark: cw.dark,
        glass: cw.cfg.glass_alpha(),
        stripe: cw.cfg.stripe,
        hot: cw.hot,
        hovered: cw.hovered,
        clickable: cw.clickable,
        folded,
    }
}

/// Sizes cw for its content at its scale: body lines (measured with the
/// real font, so it tracks DPI), height, the x and the buttons. Past two
/// lines the body is ellipsized: a card is a glance, not a paragraph.
fn measure(cw: &mut CardWin, surf: &mut dyn Surface) {
    let sc = cw.scale;
    cw.w = px(CARD_WIDTH_DIP, sc);
    cw.radius = px(cw.cfg.corner_radius, sc);
    cw.lines = 0;
    if !cw.body.is_empty() {
        let width = cw.w - px(TEXT_LEFT_DIP, sc) - px(TEXT_RIGHT_DIP, sc);
        cw.lines = surf
            .body_lines(&cw.body, width, sc)
            .clamp(1, MAX_BODY_LINES);
    }
    let widths: Option<Vec<i32>> = cw.labels.iter().map(|l| surf.label_width(l, sc)).collect();
    let (tw, th) = cw
        .image
        .as_ref()
        .map_or((0.0, 0.0), |i| thumb_size_dip(i.w, i.h));
    cw.thumb = thumb_rect(cw.lines, tw, th, sc);
    cw.h = px(card_height_dip(cw.lines, th, !cw.labels.is_empty()), sc);
    cw.close = close_rect(cw.w, sc);
    // A failed measure still leaves the buttons clickable.
    let widths = widths.unwrap_or_else(|| vec![0; cw.labels.len()]);
    cw.buttons = button_rects(cw.w, cw.h, &widths, sc);
}
