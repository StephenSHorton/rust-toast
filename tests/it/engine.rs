//! The window engine, driven headless: what go-toast's smoke tests check
//! on a real desktop (stacking, replace in place, click actions, fades,
//! hover) and the stack's motion (decks, opening, the wheel, drags, throws)
//! against a surface that records what it is told.

use std::collections::BTreeMap;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use toast::engine::{Engine, Look, Mouse, Surface, Wheel};
use toast::layout::{CARD_MARGIN_DIP, CARD_WIDTH_DIP, Part, Rect, px};
use toast::note::{FADE_OUT, FULL_ALPHA};
use toast::{
    Action, CORNER_BOTTOM_RIGHT, CORNER_TOP_RIGHT, Config, DARK, Kind, LIGHT, Motion, Note,
    Options, Scheme, hex,
};

#[derive(Debug, Clone, Default)]
struct Win {
    look: Option<Look>,
    pos: (i32, i32, i32, i32),
    alpha: u8,
    shown: bool,
    raised: u32,
    capture_excluded: bool,
}

#[derive(Default)]
pub(crate) struct Rec {
    work: Rect,
    scale: f64,
    wins: BTreeMap<u64, Win>,
    destroyed: Vec<u64>,
    timer: bool,
    frames: bool,
    work_calls: Vec<bool>,
    pub(crate) bursts: Vec<(Rect, Rect)>,
    os_dark: bool,
    creates: u32,
    /// The last restack order, bottom to top.
    stacked: Vec<u64>,
    cursor: Option<(i32, i32)>,
    reduce: bool,
    hovered: bool,
}

impl Rec {
    pub(crate) fn new(scale: f64) -> Rec {
        Rec {
            work: Rect {
                l: 0,
                t: 0,
                r: 1920,
                b: 1040,
            },
            scale,
            ..Rec::default()
        }
    }

    /// Card key's top-left on screen.
    fn at(&self, key: u64) -> (i32, i32) {
        let p = self.wins[&key].pos;
        (p.0, p.1)
    }

    /// Screen point (x, y) in card key's coordinates, as the OS gives them.
    fn local(&self, key: u64, x: i32, y: i32) -> (i32, i32) {
        let (l, t) = self.at(key);
        (x - l, y - t)
    }
}

impl Surface for Rec {
    fn work_area(&mut self, active: bool, _old: Rect) -> (Rect, f64) {
        self.work_calls.push(active);
        (self.work, self.scale)
    }
    fn body_lines(&mut self, body: &str, width: i32, scale: f64) -> usize {
        // About 7 dip a character.
        let per = (width as f64 / (7.0 * scale)).max(1.0) as usize;
        body.chars().count().div_ceil(per)
    }
    fn label_width(&mut self, label: &str, scale: f64) -> Option<i32> {
        Some((label.chars().count() as f64 * 7.0 * scale) as i32)
    }
    fn create(&mut self, key: u64, cfg: &Config, _w: i32, _h: i32) -> bool {
        self.creates += 1;
        self.wins.insert(
            key,
            Win {
                capture_excluded: cfg.capture_exclude,
                ..Win::default()
            },
        );
        true
    }
    fn draw(&mut self, look: &Look) {
        self.wins.get_mut(&look.key).unwrap().look = Some(look.clone());
    }
    fn place(&mut self, key: u64, x: i32, y: i32, w: i32, h: i32) {
        self.wins.get_mut(&key).unwrap().pos = (x, y, w, h);
    }
    fn restack(&mut self, keys: &[u64]) {
        for k in keys {
            self.wins.get_mut(k).unwrap().raised += 1;
        }
        self.stacked = keys.to_vec();
    }
    fn alpha(&mut self, key: u64, a: u8) {
        self.wins.get_mut(&key).unwrap().alpha = a;
    }
    fn show(&mut self, key: u64) {
        self.wins.get_mut(&key).unwrap().shown = true;
    }
    fn destroy(&mut self, key: u64) {
        self.wins.remove(&key);
        self.destroyed.push(key);
    }
    fn timer(&mut self, on: bool) {
        self.timer = on;
    }
    fn frames(&mut self, on: bool) {
        self.frames = on;
    }
    fn cursor(&mut self) -> Option<(i32, i32)> {
        self.cursor
    }
    fn reduce_motion(&mut self) -> bool {
        self.reduce
    }
    fn hovered(&mut self, open: bool) {
        self.hovered = open;
    }
    fn confetti(&mut self, card: Rect, work: Rect) {
        self.bursts.push((card, work));
    }
    fn os_dark(&mut self) -> bool {
        self.os_dark
    }
}

fn note(title: &str) -> Note {
    Note {
        title: title.into(),
        ..Note::default()
    }
}

fn engine() -> Engine {
    let e = Engine::new();
    e.configure(Config {
        app_name: "Irori".into(),
        ..Config::default()
    });
    e
}

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

/// Long enough for any move to land.
const SETTLE: Duration = Duration::from_millis(1000);

#[test]
fn cards_stack_replace_and_place_in_the_corner() {
    let e = engine();
    let mut s = Rec::new(1.5);
    let t0 = Instant::now();
    e.post(
        Note {
            id: "save".into(),
            title: "Saving clip…".into(),
            body: "Test · Last 1 min".into(),
            silent: true,
            ..Note::default()
        },
        t0,
    );
    e.sync(&mut s, t0);
    assert_eq!(
        s.work_calls,
        vec![true],
        "a new stack goes to the active screen"
    );
    e.post(note("Clip saved"), t0);
    e.post(
        Note {
            id: "save".into(),
            title: "Copied to clipboard".into(),
            kind: Kind::Success,
            ..Note::default()
        },
        t0,
    );
    e.sync(&mut s, t0);
    assert_eq!(
        s.work_calls,
        vec![true, false],
        "a stack that is up rehomes"
    );
    let keys = e.keys();
    assert_eq!(keys.len(), 2, "same id replaces in place");
    e.frame(&mut s, t0 + SETTLE);
    let first = s.wins[&keys[0]].look.clone().unwrap();
    assert_eq!(first.title, "Copied to clipboard");
    assert_eq!(first.lines, 0, "the replace dropped the body");
    assert_eq!(first.h, px(44.0, 1.5));
    // Oldest nearest the top-right corner, newest farther out, both at the
    // right margin.
    let w = px(CARD_WIDTH_DIP, 1.5);
    let m = px(CARD_MARGIN_DIP, 1.5);
    let oldest = s.wins[&keys[0]].pos;
    let newest = s.wins[&keys[1]].pos;
    assert_eq!((oldest.0, oldest.1, oldest.2), (1920 - m - w, m, w));
    assert_eq!(newest.1, m + oldest.3 + px(8.0, 1.5));
    assert!(s.wins.values().all(|w| w.shown && w.capture_excluded));
    assert!(s.timer, "timed cards tick");
}

#[test]
fn slides_in_expires_slides_out_and_destroys() {
    let e = engine();
    let mut s = Rec::new(1.0);
    let t0 = Instant::now();
    e.post(note("x"), t0);
    e.sync(&mut s, t0);
    let k = e.keys()[0];
    let (slot, folded) = e.slot(k).unwrap();
    assert!(!folded);
    assert_eq!(s.wins[&k].alpha, 0, "fades in from nothing");
    let start = s.wins[&k].pos;
    assert!(
        start.0 > slot.l && start.2 < slot.r - slot.l,
        "from the screen edge, a little smaller: {start:?} vs {slot:?}"
    );
    assert!(s.frames, "frames while it moves");
    e.frame(&mut s, t0 + ms(420));
    assert_eq!(s.wins[&k].alpha, FULL_ALPHA);
    assert_eq!(s.wins[&k].pos.0, slot.l, "landed");
    assert!(!s.frames, "no frames at rest");

    let end = t0 + Duration::from_secs(3);
    e.tick(end);
    e.sync(&mut s, end);
    assert!(
        e.keys().is_empty(),
        "out of the stack as it starts to leave"
    );
    e.frame(&mut s, end + FADE_OUT / 2);
    assert!(s.wins[&k].alpha < FULL_ALPHA, "fading");
    assert!(s.wins[&k].pos.0 > slot.l, "toward the edge");
    e.tick(end + FADE_OUT);
    e.sync(&mut s, end + FADE_OUT);
    assert!(e.visible(), "its exit is still playing");
    e.frame(&mut s, end + ms(400));
    assert!(s.wins.is_empty() && s.destroyed == vec![k]);
    assert!(!s.timer, "the timer stops with the last card");
    assert!(!s.frames);
    assert!(!e.visible());
}

#[test]
fn sticky_cards_need_no_tick() {
    let e = engine();
    let mut s = Rec::new(1.0);
    let t0 = Instant::now();
    e.post(
        Note {
            sticky: true,
            ..note("held")
        },
        t0,
    );
    e.sync(&mut s, t0);
    assert!(!s.timer, "only sticky cards: no tick");
    assert!(s.frames, "sliding in");
    e.frame(&mut s, t0 + SETTLE);
    assert!(!s.frames, "at rest");
}

#[test]
fn buttons_and_clicks_run_once_on_their_own_thread() {
    let e = engine();
    let mut s = Rec::new(1.0);
    let t0 = Instant::now();
    let (tx, rx) = mpsc::channel::<String>();
    let (t1, t2, t3) = (tx.clone(), tx.clone(), tx.clone());
    e.post(
        Note {
            title: "Irori v9.9.9 is ready".into(),
            body: "Restart now.".into(),
            sticky: true,
            actions: vec![
                Action::new("Restart", move || t1.send("restart".into()).unwrap()),
                Action::close("Later"),
            ],
            on_click: Some(std::sync::Arc::new(move || t2.send("body".into()).unwrap())),
            on_dismiss: Some(std::sync::Arc::new(move || {
                t3.send("dismiss".into()).unwrap()
            })),
            ..Note::default()
        },
        t0,
    );
    e.sync(&mut s, t0);
    let t = t0 + SETTLE;
    e.frame(&mut s, t);
    let k = e.keys()[0];
    let look = s.wins[&k].look.clone().unwrap();
    assert_eq!(look.labels, vec!["Restart", "Later"]);
    assert_eq!(look.buttons.len(), 2);
    assert_eq!(look.lines, 1);
    let b = look.buttons[0];
    // Hover, press and release on the primary button.
    assert!(!e.mouse(&mut s, k, Mouse::Move, b.l + 4, b.t + 4, t));
    let hot = s.wins[&k].look.clone().unwrap();
    assert!(hot.hovered && hot.hot == Part::Button(0) && hot.hand_cursor());
    assert!(!e.mouse(&mut s, k, Mouse::Down, b.l + 4, b.t + 4, t));
    assert!(
        e.mouse(&mut s, k, Mouse::Up, b.l + 4, b.t + 4, t),
        "a click asks for a sync"
    );
    assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), "restart");
    e.sync(&mut s, t);
    assert!(e.keys().is_empty());
    e.frame(&mut s, t + SETTLE);
    assert!(s.wins.is_empty(), "the clicked card went");
    assert!(
        rx.recv_timeout(Duration::from_millis(50)).is_err(),
        "only one outcome"
    );
}

#[test]
fn press_dragged_off_is_no_click_and_right_click_dismisses() {
    let e = engine();
    let mut s = Rec::new(1.0);
    let t0 = Instant::now();
    let (tx, rx) = mpsc::channel::<&str>();
    let t1 = tx.clone();
    e.post(
        Note {
            sticky: true,
            on_dismiss: Some(std::sync::Arc::new(move || t1.send("dismiss").unwrap())),
            ..note("x")
        },
        t0,
    );
    e.sync(&mut s, t0);
    let t = t0 + SETTLE;
    e.frame(&mut s, t);
    let k = e.keys()[0];
    e.mouse(&mut s, k, Mouse::Down, 50, 20, t);
    assert!(
        !e.mouse(&mut s, k, Mouse::Up, 50, 400, t),
        "released off the card"
    );
    // A body click with no on_click is a dismiss.
    e.mouse(&mut s, k, Mouse::Down, 50, 20, t);
    assert!(e.mouse(&mut s, k, Mouse::Up, 50, 20, t));
    assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), "dismiss");

    let t2 = tx.clone();
    e.post(
        Note {
            sticky: true,
            on_dismiss: Some(std::sync::Arc::new(move || t2.send("right").unwrap())),
            ..note("y")
        },
        t,
    );
    e.sync(&mut s, t);
    let k = e.keys()[0];
    assert!(e.mouse(&mut s, k, Mouse::RightUp, 5, 5, t));
    assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), "right");
}

#[test]
fn hover_pauses_expiry_and_leave_restarts_it() {
    let e = engine();
    let mut s = Rec::new(1.0);
    let t0 = Instant::now();
    e.post(note("x"), t0);
    e.sync(&mut s, t0);
    e.frame(&mut s, t0 + SETTLE);
    let k = e.keys()[0];
    e.mouse(&mut s, k, Mouse::Move, 30, 20, t0 + SETTLE);
    assert!(e.is_open());
    let late = t0 + Duration::from_secs(60);
    e.tick(late);
    e.sync(&mut s, late);
    assert_eq!(e.keys(), vec![k], "nothing expires while hovered");
    e.mouse(&mut s, k, Mouse::Leave, 0, 0, late);
    assert!(!e.is_open(), "leaving folds at once");
    assert!(!s.wins[&k].look.clone().unwrap().hovered);
    e.tick(late + Duration::from_millis(1400));
    e.sync(&mut s, late + Duration::from_millis(1400));
    assert_eq!(
        s.wins[&k].alpha, FULL_ALPHA,
        "at least 1.5 s after the cursor leaves"
    );
    let gone = late + Duration::from_millis(1500);
    e.tick(gone);
    e.tick(gone + FADE_OUT);
    e.sync(&mut s, gone + FADE_OUT);
    e.frame(&mut s, gone + FADE_OUT + SETTLE);
    assert!(s.wins.is_empty());
}

#[test]
fn close_and_options_off_clear_without_callbacks() {
    let e = engine();
    let mut s = Rec::new(1.0);
    let t0 = Instant::now();
    let (tx, rx) = mpsc::channel::<&str>();
    let t1 = tx.clone();
    e.post(
        Note {
            id: "a".into(),
            sticky: true,
            on_dismiss: Some(std::sync::Arc::new(move || t1.send("ran").unwrap())),
            ..note("a")
        },
        t0,
    );
    e.post(note("b"), t0);
    e.sync(&mut s, t0);
    assert!(e.close("a") && !e.close("a"));
    e.sync(&mut s, t0);
    assert_eq!(e.keys().len(), 1);
    assert!(e.set_options(Options {
        enabled: false,
        ..Options::default()
    }));
    e.sync(&mut s, t0);
    e.frame(&mut s, t0 + SETTLE);
    assert!(s.wins.is_empty());
    assert!(
        rx.recv_timeout(Duration::from_millis(50)).is_err(),
        "no callback ran"
    );
    let p = e.post(note("off"), t0);
    assert!(!p.shown && p.sound.is_none(), "off makes a post a no-op");
}

#[test]
fn sounds_follow_kind_buttons_and_settings() {
    use toast::sound::Sound;
    let e = engine();
    let t0 = Instant::now();
    let p = e.post(
        Note {
            kind: Kind::Error,
            ..note("x")
        },
        t0,
    );
    assert_eq!(p.sound, Some((Sound::Fail, 0.6)));
    let p = e.post(
        Note {
            actions: vec![Action::close("Ok")],
            ..note("y")
        },
        t0,
    );
    assert_eq!(p.sound.map(|s| s.0), Some(Sound::Published));
    let p = e.post(
        Note {
            silent: true,
            ..note("z")
        },
        t0,
    );
    assert!(p.shown && p.sound.is_none());
    e.set_options(Options {
        sound: false,
        ..Options::default()
    });
    assert!(e.post(note("w"), t0).sound.is_none());
}

#[test]
fn a_sixth_card_drops_the_oldest_non_sticky_window() {
    let e = engine();
    let mut s = Rec::new(1.0);
    let t0 = Instant::now();
    e.post(
        Note {
            sticky: true,
            ..note("sticky")
        },
        t0,
    );
    for i in 0..5 {
        e.post(note(&format!("n{i}")), t0);
    }
    e.sync(&mut s, t0);
    let titles: Vec<String> = e
        .keys()
        .iter()
        .map(|k| s.wins[k].look.clone().unwrap().title)
        .collect();
    assert_eq!(titles, vec!["sticky", "n1", "n2", "n3", "n4"]);
}

#[test]
fn the_card_limit_is_configurable() {
    let e = Engine::new();
    e.configure(Config {
        motion: Motion {
            max_cards: 8,
            ..Motion::default()
        },
        ..Config::default()
    });
    let t0 = Instant::now();
    for i in 0..10 {
        e.post(note(&format!("n{i}")), t0);
    }
    let titles: Vec<String> = e.notes().into_iter().map(|n| n.title).collect();
    assert_eq!(titles.len(), 8);
    assert_eq!(titles[0], "n2", "the oldest went");
}

#[test]
fn two_line_bodies_grow_the_card() {
    let e = engine();
    let mut s = Rec::new(1.0);
    let t0 = Instant::now();
    e.post(
        Note {
            body: "word ".repeat(40),
            actions: vec![Action::close("Open")],
            ..note("long")
        },
        t0,
    );
    e.sync(&mut s, t0);
    e.frame(&mut s, t0 + SETTLE);
    let l = s.wins[&e.keys()[0]].look.clone().unwrap();
    assert_eq!(l.lines, 2, "clamped to two lines");
    assert_eq!(l.h, 11 + 21 + 12 + 1 + 34 + 36);
}

#[test]
fn confetti_bursts_from_placed_success_cards_only_when_on() {
    let e = engine();
    let mut s = Rec::new(1.0);
    let t0 = Instant::now();
    let ok = |title: &str| Note {
        title: title.into(),
        kind: Kind::Success,
        ..Note::default()
    };

    // Off by default: no burst.
    e.post(ok("done"), t0);
    e.sync(&mut s, t0);
    assert!(s.bursts.is_empty());

    e.set_options(Options {
        confetti: true,
        ..Options::default()
    });
    e.post(note("info"), t0);
    e.post(
        Note {
            kind: Kind::Error,
            ..note("bad")
        },
        t0,
    );
    e.sync(&mut s, t0);
    assert!(s.bursts.is_empty(), "only success cards throw confetti");

    e.post(ok("finished"), t0);
    e.sync(&mut s, t0);
    assert_eq!(s.bursts.len(), 1);
    let key = *e.keys().last().unwrap();
    // From where the card lands.
    e.frame(&mut s, t0 + SETTLE);
    let (x, y, w, h) = s.wins[&key].pos;
    let (card, work) = s.bursts[0];
    assert_eq!(
        card,
        Rect {
            l: x,
            t: y,
            r: x + w,
            b: y + h
        }
    );
    assert_eq!(work, s.work);

    // Once per post: later syncs don't repeat it.
    e.sync(&mut s, t0 + Duration::from_millis(30));
    assert_eq!(s.bursts.len(), 1);
}

// ---- the motion model: list + deck, deck in the corner ----

/// An engine with n sticky cards in corner, landed.
fn stack(n: usize, corner: &str, s: &mut Rec, t0: Instant) -> (Engine, Vec<u64>) {
    let e = Engine::new();
    e.configure(Config {
        motion: Motion {
            max_cards: 40,
            ..Motion::default()
        },
        ..Config::default()
    });
    e.set_options(Options {
        corner: corner.into(),
        ..Options::default()
    });
    for i in 0..n {
        e.post(
            Note {
                sticky: true,
                ..note(&format!("n{i}"))
            },
            t0,
        );
    }
    e.sync(s, t0);
    e.frame(s, t0 + SETTLE);
    let keys = e.keys();
    assert_eq!(keys.len(), n);
    (e, keys)
}

fn rect_of(s: &Rec, k: u64) -> Rect {
    let (x, y, w, h) = s.wins[&k].pos;
    Rect {
        l: x,
        t: y,
        r: x + w,
        b: y + h,
    }
}

#[test]
fn at_rest_older_cards_fold_into_a_deck_in_the_corner() {
    let mut s = Rec::new(1.0);
    let t0 = Instant::now();
    let (e, k) = stack(6, CORNER_BOTTOM_RIGHT, &mut s, t0);
    // 6 cards, 3 shown: 3 in the deck, the newest of them (k[2]) in front,
    // the two older ones peeking out below it, toward the corner.
    let (x, base) = (1920 - 20 - 340, 1040 - 20);
    let front = rect_of(&s, k[2]);
    assert_eq!(
        front,
        Rect {
            l: x,
            t: base - 24 - 44,
            r: x + 340,
            b: base - 24
        }
    );
    assert!(
        !s.wins[&k[2]].look.as_ref().unwrap().folded,
        "the front shows its content"
    );
    for (i, back) in [(1, 1.0), (0, 2.0)] {
        let r = rect_of(&s, k[i]);
        let l = s.wins[&k[i]].look.clone().unwrap();
        assert!(l.folded, "card {i} folded");
        let sc: f64 = 1.0 - 0.05 * back;
        assert_eq!(r.r - r.l, (340.0 * sc).round() as i32, "card {i} smaller");
        assert_eq!(
            r.b - r.t,
            (44.0 * sc).round() as i32,
            "cut to the front's height"
        );
        assert_eq!(
            s.wins[&k[i]].alpha,
            ((1.0 - 0.22 * back) * FULL_ALPHA as f64).round() as u8
        );
        assert!(r.b > front.b, "card {i} peeks out on the corner side");
        assert!(((r.l + r.r) / 2 - (x + 170)).abs() <= 1, "centred");
        assert_eq!(e.slot(k[i]), Some((r, true)));
    }
    assert!(
        rect_of(&s, k[0]).b > rect_of(&s, k[1]).b,
        "the oldest peeks farthest"
    );
    // The deck reserves its peeks and front, then the newest three run out
    // from it, newest farthest from the corner.
    let mut edge = base - 24 - 44 - 8;
    for (i, key) in k.iter().enumerate().skip(3) {
        let r = rect_of(&s, *key);
        assert_eq!(
            (r.l, r.b, r.r - r.l, r.b - r.t),
            (x, edge, 340, 44),
            "shown card {i}"
        );
        edge -= 44 + 8;
    }
    // Peeking cards behind the front, the shown ones over the deck, newer on top.
    assert_eq!(s.stacked, k, "{:?}", s.stacked);
    assert!(!s.frames && !s.hovered, "at rest");
}

#[test]
fn a_deck_peeks_at_most_its_depth() {
    let mut s = Rec::new(1.0);
    let t0 = Instant::now();
    let (_e, k) = stack(8, CORNER_TOP_RIGHT, &mut s, t0);
    // 5 folded: front k[4], k[3] and k[2] peek (above it, toward a top
    // corner), k[1] and k[0] are hidden behind.
    let front = rect_of(&s, k[4]);
    assert_eq!(front.t, 20 + 24);
    assert!(rect_of(&s, k[3]).t < front.t && rect_of(&s, k[2]).t < rect_of(&s, k[3]).t);
    // Each card behind shrinks toward the edge the front hides, so the
    // deck's back stays inside the margin.
    assert!(
        (20..30).contains(&rect_of(&s, k[2]).t),
        "{:?}",
        rect_of(&s, k[2])
    );
    for i in [0, 1] {
        assert_eq!(s.wins[&k[i]].alpha, 0, "card {i} hidden");
    }
    assert_eq!(
        rect_of(&s, k[5]).t,
        20 + 24 + 44 + 8,
        "the first shown card under the deck"
    );
}

#[test]
fn hover_opens_every_card_when_they_fit_and_leaving_folds() {
    let mut s = Rec::new(1.0);
    let t0 = Instant::now();
    let (e, k) = stack(6, CORNER_BOTTOM_RIGHT, &mut s, t0);
    let t = t0 + SETTLE;
    e.mouse(&mut s, k[5], Mouse::Move, 10, 10, t);
    assert!(e.is_open() && s.hovered && s.frames);
    assert_eq!(e.scroll(), None, "everything fits");
    e.frame(&mut s, t + SETTLE);
    let mut bottom = 1040 - 20;
    for (i, key) in k.iter().enumerate() {
        let r = rect_of(&s, *key);
        assert_eq!(
            (r.b, r.r - r.l),
            (bottom, 340),
            "card {i} in full, oldest at the corner"
        );
        assert!(!s.wins[key].look.as_ref().unwrap().folded);
        bottom -= 44 + 8;
    }
    // The cursor goes off the stack: folds at once.
    s.cursor = Some((10, 10));
    e.mouse(&mut s, k[5], Mouse::Leave, 0, 0, t + SETTLE);
    assert!(!e.is_open() && !s.hovered);
    e.frame(&mut s, t + SETTLE * 2);
    assert!(s.wins[&k[0]].look.as_ref().unwrap().folded);
    assert!(!s.frames, "at rest again");
}

#[test]
fn moving_between_cards_keeps_the_stack_open() {
    let mut s = Rec::new(1.0);
    let t0 = Instant::now();
    let (e, k) = stack(4, CORNER_BOTTOM_RIGHT, &mut s, t0);
    let t = t0 + SETTLE;
    e.mouse(&mut s, k[3], Mouse::Move, 10, 10, t);
    e.frame(&mut s, t + SETTLE);
    // In the gap between two cards: no card under the cursor, still on the stack.
    let gap = rect_of(&s, k[3]).b + 4;
    s.cursor = Some((rect_of(&s, k[3]).l + 20, gap));
    e.mouse(&mut s, k[3], Mouse::Leave, 0, 0, t + SETTLE);
    assert!(e.is_open(), "the gap is part of the stack");
    e.frame(&mut s, t + SETTLE + ms(16));
    assert!(e.is_open());
    // Then off it: the frame sees the cursor leave and folds.
    s.cursor = Some((0, 0));
    e.frame(&mut s, t + SETTLE + ms(32));
    assert!(!e.is_open());
}

/// A short work area: 12 title-only cards (52 px a row) can't all fit.
fn crowded(corner: &str, s: &mut Rec, t0: Instant) -> (Engine, Vec<u64>) {
    s.work = Rect {
        l: 0,
        t: 0,
        r: 1920,
        b: 400,
    };
    stack(12, corner, s, t0)
}

fn inside(r: Rect, work: Rect) -> bool {
    r.l >= work.l && r.t >= work.t && r.r <= work.r && r.b <= work.b
}

#[test]
fn too_many_to_fit_open_on_the_newest_with_a_deck_at_each_end() {
    let mut s = Rec::new(1.0);
    let t0 = Instant::now();
    let (e, k) = crowded(CORNER_BOTTOM_RIGHT, &mut s, t0);
    let t = t0 + SETTLE;
    e.mouse(&mut s, k[11], Mouse::Move, 10, 10, t);
    // Room 360: two decks take 2 * (52 + 24); 4 cards fit between.
    assert_eq!(e.scroll(), Some((8, 8)), "opens on the newest");
    e.frame(&mut s, t + SETTLE);
    let folded: Vec<bool> = k.iter().map(|x| e.slot(*x).unwrap().1).collect();
    assert_eq!(
        folded,
        [
            true, true, true, true, true, true, true, false, false, false, false, false
        ],
        "the older cards fold at the corner, the newest four in full"
    );
    for x in &k {
        assert!(inside(rect_of(&s, *x), s.work), "on screen");
    }

    // The wheel toward the person: older (bottom corner).
    let t = t + SETTLE;
    assert!(e.wheel(&mut s, Wheel::Down, t));
    assert_eq!(e.scroll(), Some((7, 8)));
    assert!(
        !e.wheel(&mut s, Wheel::Down, t + ms(50)),
        "within the debounce"
    );
    assert!(e.wheel(&mut s, Wheel::Down, t + ms(100)));
    assert_eq!(e.scroll(), Some((6, 8)));
    e.frame(&mut s, t + SETTLE);
    // Now a deck at the far end too, peeking away from the corner.
    let (far_front, f) = e.slot(k[10]).unwrap();
    assert!(!f, "the far deck's front, nearest the shown cards");
    let (far_back, f) = e.slot(k[11]).unwrap();
    assert!(f);
    assert!(far_back.t < far_front.t, "peeks out above it");
    for x in &k {
        assert!(inside(rect_of(&s, *x), s.work));
    }

    // Away from the person: newer, up to the newest and no further.
    let mut at = t + SETTLE;
    for want in [7, 8] {
        at += ms(100);
        assert!(e.wheel(&mut s, Wheel::Up, at));
        assert_eq!(e.scroll(), Some((want, 8)));
    }
    assert!(
        !e.wheel(&mut s, Wheel::Up, at + ms(100)),
        "clamped at the newest"
    );
}

#[test]
fn in_a_top_corner_the_wheel_up_goes_older() {
    let mut s = Rec::new(1.0);
    let t0 = Instant::now();
    let (e, k) = crowded(CORNER_TOP_RIGHT, &mut s, t0);
    let t = t0 + SETTLE;
    e.mouse(&mut s, k[11], Mouse::Move, 10, 10, t);
    assert_eq!(e.scroll(), Some((8, 8)));
    assert!(e.wheel(&mut s, Wheel::Up, t));
    assert_eq!(e.scroll(), Some((7, 8)));
    assert!(!e.wheel(&mut s, Wheel::Down, t + ms(10)), "debounced");
}

#[test]
fn the_wheel_does_nothing_when_everything_fits_or_at_rest() {
    let mut s = Rec::new(1.0);
    let t0 = Instant::now();
    let (e, k) = stack(4, CORNER_BOTTOM_RIGHT, &mut s, t0);
    assert!(!e.wheel(&mut s, Wheel::Up, t0 + SETTLE), "at rest");
    e.mouse(&mut s, k[3], Mouse::Move, 10, 10, t0 + SETTLE);
    assert!(!e.wheel(&mut s, Wheel::Up, t0 + SETTLE), "all of them fit");
}

#[test]
fn mixed_heights_never_run_off_the_screen() {
    let mut s = Rec::new(1.0);
    s.work = Rect {
        l: 0,
        t: 0,
        r: 1920,
        b: 520,
    };
    let t0 = Instant::now();
    let e = Engine::new();
    e.configure(Config {
        motion: Motion {
            max_cards: 40,
            ..Motion::default()
        },
        ..Config::default()
    });
    for i in 0..14 {
        let tall = i % 3 == 0;
        e.post(
            Note {
                sticky: true,
                body: if tall {
                    "word ".repeat(30)
                } else {
                    String::new()
                },
                actions: if tall {
                    vec![Action::close("Open")]
                } else {
                    vec![]
                },
                ..note(&format!("n{i}"))
            },
            t0,
        );
    }
    e.sync(&mut s, t0);
    let k = e.keys();
    let mut t = t0 + SETTLE;
    e.mouse(&mut s, k[13], Mouse::Move, 10, 10, t);
    let (_, max) = e.scroll().expect("too many to fit");
    for _ in 0..=max {
        e.frame(&mut s, t + SETTLE);
        for x in &k {
            assert!(
                inside(rect_of(&s, *x), s.work),
                "{:?} at {:?}",
                rect_of(&s, *x),
                e.scroll()
            );
        }
        t += SETTLE;
        e.wheel(&mut s, Wheel::Up, t); // top-right: older
    }
    assert_eq!(e.scroll().unwrap().0, 0, "walked to the oldest");
}

/// Presses card k on its body and drags it through (dx, dy) screen
/// offsets, a move every `every`; returns the time of the last move.
fn drag(
    e: &Engine,
    s: &mut Rec,
    k: u64,
    steps: &[(i32, i32)],
    every: Duration,
    t: Instant,
) -> Instant {
    let (l, top) = s.at(k);
    let (x0, y0) = (l + 60, top + 20);
    e.mouse(s, k, Mouse::Down, 60, 20, t);
    let mut now = t;
    for (dx, dy) in steps {
        now += every;
        let (x, y) = s.local(k, x0 + dx, y0 + dy);
        e.mouse(s, k, Mouse::Move, x, y, now);
    }
    now
}

/// Releases a drag started by [`drag`] on a card that was at `start`.
fn release(
    e: &Engine,
    s: &mut Rec,
    k: u64,
    dx: i32,
    dy: i32,
    start: (i32, i32),
    t: Instant,
) -> bool {
    let (x, y) = s.local(k, start.0 + 60 + dx, start.1 + 20 + dy);
    e.mouse(s, k, Mouse::Up, x, y, t)
}

fn dismissable(n: usize, s: &mut Rec, t0: Instant) -> (Engine, Vec<u64>, mpsc::Receiver<u64>) {
    let e = Engine::new();
    e.configure(Config {
        motion: Motion {
            max_cards: 40,
            ..Motion::default()
        },
        ..Config::default()
    });
    e.set_options(Options {
        corner: CORNER_BOTTOM_RIGHT.into(),
        ..Options::default()
    });
    let (tx, rx) = mpsc::channel();
    for i in 0..n {
        let tx = tx.clone();
        e.post(
            Note {
                sticky: true,
                on_dismiss: Some(std::sync::Arc::new(move || tx.send(i as u64).unwrap())),
                ..note(&format!("n{i}"))
            },
            t0,
        );
    }
    e.sync(s, t0);
    e.frame(s, t0 + SETTLE);
    let k = e.keys();
    (e, k, rx)
}

#[test]
fn a_sideways_drag_follows_and_springs_back_short_of_the_line() {
    let mut s = Rec::new(1.0);
    let t0 = Instant::now();
    let (e, k, rx) = dismissable(1, &mut s, t0);
    let start = s.at(k[0]);
    let t = drag(
        &e,
        &mut s,
        k[0],
        &[(20, 2), (60, 4), (100, 6)],
        ms(100),
        t0 + SETTLE,
    );
    assert_eq!(
        s.at(k[0]),
        (start.0 + 100, start.1),
        "follows sideways only"
    );
    let fade = 1.0 - 100.0 / 260.0;
    assert_eq!(
        s.wins[&k[0]].alpha,
        (fade * FULL_ALPHA as f64).round() as u8
    );
    // 100 < 110 px, and slowly: back it goes, and it's no click either.
    assert!(!release(&e, &mut s, k[0], 100, 6, start, t + ms(100)));
    e.frame(&mut s, t + SETTLE);
    assert_eq!(s.at(k[0]), start);
    assert_eq!(e.keys(), k);
    assert!(rx.try_recv().is_err());
}

#[test]
fn past_the_line_a_card_is_thrown_away_as_a_dismiss() {
    let mut s = Rec::new(1.0);
    let t0 = Instant::now();
    let (e, k, rx) = dismissable(2, &mut s, t0);
    let start = s.at(k[1]);
    let t = drag(
        &e,
        &mut s,
        k[1],
        &[(-20, 0), (-80, 0), (-120, 0)],
        ms(200),
        t0 + SETTLE,
    );
    assert!(
        release(&e, &mut s, k[1], -120, 0, start, t + ms(200)),
        "sync"
    );
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(2)).unwrap(),
        1,
        "its dismiss ran"
    );
    e.sync(&mut s, t + ms(200));
    assert_eq!(e.keys(), vec![k[0]]);
    e.frame(&mut s, t + ms(400));
    assert!(s.at(k[1]).0 < start.0 - 120, "flying off to the left");
    assert_eq!(*s.stacked.last().unwrap(), k[1], "over the rest");
    e.frame(&mut s, t + SETTLE);
    assert!(!s.wins.contains_key(&k[1]));
}

#[test]
fn a_quick_flick_throws_it_regardless_of_distance() {
    let mut s = Rec::new(1.0);
    let t0 = Instant::now();
    let (e, k, rx) = dismissable(1, &mut s, t0);
    let start = s.at(k[0]);
    // 15 px in 10 ms: 1.5 px/ms, past 0.6.
    let t = drag(&e, &mut s, k[0], &[(15, 0), (30, 0)], ms(10), t0 + SETTLE);
    assert!(release(&e, &mut s, k[0], 30, 0, start, t));
    assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), 0);
    e.sync(&mut s, t);
    e.frame(&mut s, t + ms(200));
    assert!(s.at(k[0]).0 > start.0 + 30, "thrown to the right");
}

#[test]
fn a_move_at_the_same_point_keeps_the_flick() {
    let mut s = Rec::new(1.0);
    let t0 = Instant::now();
    let (e, k, rx) = dismissable(1, &mut s, t0);
    let start = s.at(k[0]);
    let t = drag(&e, &mut s, k[0], &[(15, 0), (30, 0)], ms(10), t0 + SETTLE);
    // Windows' move for the window moving under a still cursor, 30 ms on.
    let (x, y) = s.local(k[0], start.0 + 60 + 30, start.1 + 20);
    e.mouse(&mut s, k[0], Mouse::Move, x, y, t + ms(30));
    assert!(release(&e, &mut s, k[0], 30, 0, start, t + ms(30)));
    assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), 0);
}

#[test]
fn coming_back_through_a_gap_cancels_the_fold_after_a_throw() {
    let mut s = Rec::new(1.0);
    let t0 = Instant::now();
    let (e, k, _rx) = dismissable(5, &mut s, t0);
    let t = t0 + SETTLE;
    e.mouse(&mut s, k[4], Mouse::Move, 10, 10, t);
    e.frame(&mut s, t + SETTLE);
    let t = t + SETTLE;
    let start = s.at(k[4]);
    let t = drag(&e, &mut s, k[4], &[(40, 0), (150, 0)], ms(200), t);
    s.cursor = Some((start.0 + 210, start.1 + 20));
    assert!(release(&e, &mut s, k[4], 150, 0, start, t));
    e.sync(&mut s, t);
    e.frame(&mut s, t + SETTLE);
    // Into the gap between k[2] and k[3]: no card under it, on the stack.
    let r = rect_of(&s, k[3]);
    s.cursor = Some((r.l + 10, r.b + 4));
    e.frame(&mut s, t + ms(1100));
    e.frame(&mut s, t + ms(1300));
    assert!(e.is_open(), "came back between cards: stays open");
}

#[test]
fn the_first_move_past_the_lock_decides_the_axis_for_good() {
    let mut s = Rec::new(1.0);
    let t0 = Instant::now();
    let (e, k, rx) = dismissable(1, &mut s, t0);
    let start = s.at(k[0]);
    // Down first: nothing to scroll, so a rubber band; turning sideways
    // later doesn't make it a dismiss.
    let t = drag(
        &e,
        &mut s,
        k[0],
        &[(2, 40), (200, 40)],
        ms(100),
        t0 + SETTLE,
    );
    assert_eq!(
        s.at(k[0]),
        (start.0, start.1 + 6),
        "0.15 of the drag, never sideways"
    );
    assert!(!release(&e, &mut s, k[0], 200, 40, start, t + ms(100)));
    e.frame(&mut s, t + SETTLE);
    assert_eq!(s.at(k[0]), start, "back");
    assert!(rx.try_recv().is_err(), "not thrown");
    assert_eq!(e.keys(), k);
}

#[test]
fn a_vertical_drag_scrolls_the_open_stack_a_card_per_step() {
    let mut s = Rec::new(1.0);
    let t0 = Instant::now();
    let (e, k) = crowded(CORNER_BOTTOM_RIGHT, &mut s, t0);
    let t = t0 + SETTLE;
    e.mouse(&mut s, k[11], Mouse::Move, 10, 10, t);
    e.frame(&mut s, t + SETTLE);
    let t = t + SETTLE;
    let slot = e.slot(k[10]).unwrap().0;
    let start = s.at(k[10]);
    // Up the screen: 13 px decides it, then each 44 px is a card, older
    // in a bottom corner. The card is let go at once.
    let t = drag(
        &e,
        &mut s,
        k[10],
        &[(0, -13), (0, -50), (0, -90)],
        ms(50),
        t,
    );
    assert_eq!(e.scroll(), Some((6, 8)), "two steps");
    e.frame(&mut s, t + SETTLE);
    assert_eq!(
        rect_of(&s, k[10]),
        e.slot(k[10]).unwrap().0,
        "in its (new) slot, not under the pointer"
    );
    assert_ne!(e.slot(k[10]).unwrap().0, slot, "the window moved");
    // And back down past where it started.
    let (x, y) = s.local(k[10], start.0 + 60, start.1 + 20 + 1);
    e.mouse(&mut s, k[10], Mouse::Move, x, y, t + SETTLE);
    assert_eq!(e.scroll(), Some((8, 8)));
}

#[test]
fn after_a_throw_the_stack_stays_open_a_moment() {
    let mut s = Rec::new(1.0);
    let t0 = Instant::now();
    let (e, k, _rx) = dismissable(5, &mut s, t0);
    let t = t0 + SETTLE;
    e.mouse(&mut s, k[4], Mouse::Move, 10, 10, t);
    e.frame(&mut s, t + SETTLE);
    let t = t + SETTLE;
    let start = s.at(k[4]);
    let t = drag(&e, &mut s, k[4], &[(40, 0), (150, 0)], ms(200), t);
    // The throw carries the pointer off the stack.
    s.cursor = Some((start.0 + 210, start.1 + 20));
    assert!(release(&e, &mut s, k[4], 150, 0, start, t));
    e.sync(&mut s, t);
    assert!(e.is_open(), "held open after the throw");
    e.frame(&mut s, t + ms(1100));
    assert!(e.is_open());
    e.frame(&mut s, t + ms(1200));
    assert!(!e.is_open(), "folds when the hold runs out");

    // Coming back within the hold cancels the fold.
    let t = t + SETTLE * 2;
    e.mouse(&mut s, k[3], Mouse::Move, 10, 10, t);
    e.frame(&mut s, t + SETTLE);
    let t = t + SETTLE;
    let start = s.at(k[3]);
    let t = drag(&e, &mut s, k[3], &[(40, 0), (150, 0)], ms(200), t);
    s.cursor = Some((start.0 + 210, start.1 + 20));
    assert!(release(&e, &mut s, k[3], 150, 0, start, t));
    e.sync(&mut s, t);
    assert!(e.is_open());
    let back = s.at(k[2]);
    s.cursor = Some((back.0 + 10, back.1 + 10));
    e.mouse(&mut s, k[2], Mouse::Move, 10, 10, t + ms(500));
    e.frame(&mut s, t + ms(1300));
    assert!(e.is_open(), "came back: stays open");
}

#[test]
fn leaving_without_a_throw_folds_at_once() {
    let mut s = Rec::new(1.0);
    let t0 = Instant::now();
    let (e, k, _rx) = dismissable(5, &mut s, t0);
    e.mouse(&mut s, k[4], Mouse::Move, 10, 10, t0 + SETTLE);
    s.cursor = Some((0, 0));
    e.mouse(&mut s, k[4], Mouse::Leave, 0, 0, t0 + SETTLE + ms(10));
    assert!(!e.is_open());
}

#[test]
fn folded_cards_take_no_clicks() {
    let mut s = Rec::new(1.0);
    let t0 = Instant::now();
    let (e, k, rx) = dismissable(5, &mut s, t0);
    // k[0] peeks behind the deck's front. A press on it before the stack
    // opens (no move first) does nothing.
    assert!(e.slot(k[0]).unwrap().1);
    let t = t0 + SETTLE;
    e.mouse(&mut s, k[0], Mouse::Down, 30, 5, t);
    assert!(!e.mouse(&mut s, k[0], Mouse::Up, 30, 5, t));
    assert!(!e.mouse(&mut s, k[0], Mouse::RightUp, 30, 5, t));
    assert!(rx.try_recv().is_err());
    assert_eq!(e.keys().len(), 5);
}

#[test]
fn expiry_waits_while_the_stack_is_open() {
    let mut s = Rec::new(1.0);
    let t0 = Instant::now();
    let e = engine();
    for i in 0..4 {
        e.post(note(&format!("n{i}")), t0);
    }
    e.sync(&mut s, t0);
    let k = e.keys();
    e.mouse(&mut s, k[3], Mouse::Move, 10, 10, t0 + ms(100));
    // The cursor sits in a gap: no card under it, the stack still open.
    e.frame(&mut s, t0 + SETTLE);
    let r = rect_of(&s, k[3]);
    s.cursor = Some((r.l + 5, r.t - 2)); // between it and the one before
    e.mouse(&mut s, k[3], Mouse::Leave, 0, 0, t0 + SETTLE);
    let late = t0 + Duration::from_secs(30);
    e.tick(late);
    e.sync(&mut s, late);
    assert_eq!(e.keys(), k, "nothing expired");
}

#[test]
fn reduce_motion_jumps_to_the_end() {
    let mut s = Rec::new(1.0);
    s.reduce = true;
    let t0 = Instant::now();
    let e = engine();
    e.post(note("a"), t0);
    e.post(
        Note {
            sticky: true,
            ..note("b")
        },
        t0,
    );
    e.sync(&mut s, t0);
    let k = e.keys();
    for key in &k {
        assert_eq!(
            rect_of(&s, *key),
            e.slot(*key).unwrap().0,
            "already in place"
        );
        assert_eq!(s.wins[key].alpha, FULL_ALPHA);
    }
    assert!(!s.frames, "nothing to animate");
    // Opening jumps too.
    e.mouse(&mut s, k[1], Mouse::Move, 10, 10, t0);
    assert!(s.frames, "open: frames watch the cursor");
    // A click takes it down at once.
    e.mouse(&mut s, k[1], Mouse::Down, 30, 20, t0);
    assert!(e.mouse(&mut s, k[1], Mouse::Up, 30, 20, t0));
    e.sync(&mut s, t0);
    assert!(!s.wins.contains_key(&k[1]), "gone, no exit");
}

#[test]
fn a_new_card_glides_the_others_and_lands_farthest_out() {
    let mut s = Rec::new(1.0);
    let t0 = Instant::now();
    let (e, k) = stack(4, CORNER_BOTTOM_RIGHT, &mut s, t0);
    let before: Vec<Rect> = k.iter().map(|x| rect_of(&s, *x)).collect();
    let t = t0 + SETTLE;
    e.post(note("new"), t);
    e.sync(&mut s, t);
    let n = *e.keys().last().unwrap();
    // k[1] folds into the corner deck as its front, k[0] peeks behind it,
    // and the shown cards glide toward the corner to close up.
    e.frame(&mut s, t + ms(100));
    let mid = rect_of(&s, k[2]);
    assert!(mid.b > before[2].b, "on its way down");
    e.frame(&mut s, t + SETTLE);
    let after = rect_of(&s, k[2]);
    assert!(after.b > mid.b, "still going at 100 ms");
    assert_eq!(after, e.slot(k[2]).unwrap().0);
    assert_eq!(
        rect_of(&s, k[1]).b,
        before[1].b + 52 - 12,
        "one peek up from the corner"
    );
    let (slot, _) = e.slot(n).unwrap();
    assert_eq!(rect_of(&s, n), slot);
    assert!(
        slot.b < rect_of(&s, k[3]).t,
        "the newest is farthest from the corner"
    );
}

#[test]
fn ease_starts_at_zero_overshoots_and_lands_on_one() {
    use toast::layout::ease;
    let m = Motion::default();
    assert_eq!(ease(m.ease, 0.0), 0.0);
    assert_eq!(ease(m.ease, 1.0), 1.0);
    let peak = (1..100)
        .map(|i| ease(m.ease, i as f64 / 100.0))
        .fold(0.0, f64::max);
    assert!(peak > 1.0, "a spring's overshoot: {peak}");
    let out = (1..100)
        .map(|i| ease(m.ease_out, i as f64 / 100.0))
        .fold(0.0, f64::max);
    assert!(out <= 1.0 + 1e-9, "no overshoot on fades");
    assert!(
        (ease([0.0, 0.0, 1.0, 1.0], 0.3) - 0.3).abs() < 1e-6,
        "linear"
    );
}

#[test]
fn motion_settings_normalize() {
    let m = Motion {
        shown: 0,
        max_cards: 0,
        peek: f64::NAN,
        peek_shrink: 3.0,
        scroll_step: 0.0,
        ease: [2.0, 0.0, -1.0, 1.0],
        ..Motion::default()
    }
    .normalized();
    assert_eq!((m.shown, m.max_cards), (1, 1));
    assert_eq!(m.peek, 12.0);
    assert_eq!(m.peek_shrink, 1.0);
    assert_eq!(m.scroll_step, 44.0);
    assert_eq!(m.ease, [1.0, 0.0, 0.0, 1.0]);
}

#[test]
fn a_thumbnail_goes_under_the_body_and_grows_the_card() {
    use toast::Image;
    use toast::layout::{TEXT_LEFT_DIP, thumb_size_dip};
    let e = engine();
    let mut s = Rec::new(2.0);
    let t0 = Instant::now();
    let plain = Note {
        id: "plain".into(),
        title: "Finished".into(),
        body: "Ready for you".into(),
        ..Note::default()
    };
    let img = Image::new(800, 400, vec![0u8; 800 * 400 * 4]).unwrap();
    e.post(plain.clone(), t0);
    e.post(
        Note {
            id: "pic".into(),
            image: Some(img.clone()),
            ..plain
        },
        t0,
    );
    e.sync(&mut s, t0);
    e.frame(&mut s, t0 + SETTLE); // landed: drawn at full size
    let looks: Vec<Look> = s.wins.values().map(|w| w.look.clone().unwrap()).collect();
    let (a, b) = (&looks[0], &looks[1]);
    assert!(a.image.is_none());
    let (got, r) = b.image.clone().unwrap();
    assert_eq!(got, img, "the same pixels, not a copy");
    let (tw, th) = thumb_size_dip(800, 400);
    assert_eq!((r.r - r.l, r.b - r.t), (px(tw, 2.0), px(th, 2.0)));
    assert_eq!(r.l, px(TEXT_LEFT_DIP, 2.0));
    assert_eq!(b.lines, a.lines);
    assert!(r.t > px(11.0 + 21.0, 2.0), "under the title and body");
    assert!(r.b < b.h, "inside the card");
    assert_eq!(b.h - a.h, px(8.0 + th, 2.0), "taller by gap + thumbnail");
}

#[test]
fn a_thumbnail_sits_above_the_buttons() {
    use toast::Image;
    let e = engine();
    let mut s = Rec::new(1.0);
    let t0 = Instant::now();
    e.post(
        Note {
            title: "Finished".into(),
            body: "Ready".into(),
            image: Image::new(100, 100, vec![255u8; 100 * 100 * 4]),
            actions: vec![Action::close("Open"), Action::close("Later")],
            ..Note::default()
        },
        t0,
    );
    e.sync(&mut s, t0);
    let l = s.wins.values().next().unwrap().look.clone().unwrap();
    let (_, r) = l.image.unwrap();
    assert!(r.b <= l.buttons[0].t, "{r:?} over {:?}", l.buttons[0]);
}

// ---- card #527: the cards follow the app's look ----

fn themed(light: toast::Palette) -> Config {
    Config {
        app_name: "Irori".into(),
        light,
        scheme: Scheme::Light,
        corner_radius: 14.0,
        glass: 0.62,
        ..Config::default()
    }
}

#[test]
fn configure_restyles_cards_on_screen() {
    let e = engine();
    let mut s = Rec::new(1.0);
    let t0 = Instant::now();
    e.post(note("Themed"), t0);
    e.sync(&mut s, t0);
    let key = *s.wins.keys().next().unwrap();
    let first = s.wins[&key].look.clone().unwrap();
    assert_eq!(first.palette, DARK.resolved());
    assert!(first.dark && first.glass.is_none() && first.radius == 0);

    let mut light = LIGHT;
    light.background = hex("#f1f2f6");
    light.tint = hex("#fcfcff");
    e.configure(themed(light));
    e.sync(&mut s, t0);
    assert_eq!(s.creates, 1, "restyled in place, not remade");
    let l = s.wins[&key].look.clone().unwrap();
    assert!(!l.dark, "light now");
    assert_eq!(l.palette.background, hex("#f1f2f6"));
    assert_eq!(l.palette.tint, hex("#fcfcff"));
    assert_eq!(l.radius, 14);
    assert_eq!(l.glass, Some(0.62));
    assert_eq!((l.title.as_str(), l.w, l.h), ("Themed", first.w, first.h));

    // The same config again changes nothing.
    s.wins.get_mut(&key).unwrap().look = None;
    assert!(!e.configure(themed(light)), "unchanged");
    e.sync(&mut s, t0);
    assert!(
        s.wins[&key].look.is_none(),
        "no redraw for an unchanged look"
    );
}

#[test]
fn system_scheme_follows_the_os() {
    let e = engine();
    e.configure(Config {
        app_name: "Irori".into(),
        scheme: Scheme::System,
        ..Config::default()
    });
    let mut s = Rec::new(1.0);
    s.os_dark = false;
    let t0 = Instant::now();
    e.post(note("Follows"), t0);
    e.sync(&mut s, t0);
    let key = *s.wins.keys().next().unwrap();
    assert!(!s.wins[&key].look.as_ref().unwrap().dark);
    assert_eq!(
        s.wins[&key].look.as_ref().unwrap().palette,
        LIGHT.resolved()
    );
    s.os_dark = true; // the OS flips; the platform syncs
    e.sync(&mut s, t0);
    let l = s.wins[&key].look.clone().unwrap();
    assert!(l.dark);
    assert_eq!(l.palette, DARK.resolved());
    assert_eq!(s.creates, 1);
}

#[test]
fn the_stripe_is_on_by_default_and_configurable() {
    for (stripe, want) in [(None, true), (Some(false), false)] {
        let e = Engine::new();
        let mut cfg = Config {
            app_name: "Demo".into(),
            ..Config::default()
        };
        if let Some(on) = stripe {
            cfg.stripe = on;
        }
        e.configure(cfg);
        let mut s = Rec::new(1.0);
        let t0 = Instant::now();
        e.post(note("x"), t0);
        e.sync(&mut s, t0);
        let look = s
            .wins
            .values()
            .next()
            .and_then(|w| w.look.clone())
            .expect("drawn");
        assert_eq!(look.stripe, want);
    }
}

// ---- review additions: timers and hooks stop, Cancel, both clamps, a card
// sliding under a still cursor ----

#[test]
fn the_frames_and_the_wheel_hook_stop_after_the_throw_hold() {
    let mut s = Rec::new(1.0);
    let t0 = Instant::now();
    let (e, k, _rx) = dismissable(5, &mut s, t0);
    let t = t0 + SETTLE;
    e.mouse(&mut s, k[4], Mouse::Move, 10, 10, t);
    e.frame(&mut s, t + SETTLE);
    let t = t + SETTLE;
    let start = s.at(k[4]);
    let t = drag(&e, &mut s, k[4], &[(40, 0), (150, 0)], ms(200), t);
    s.cursor = Some((start.0 + 210, start.1 + 20));
    assert!(release(&e, &mut s, k[4], 150, 0, start, t));
    e.sync(&mut s, t);
    assert!(s.frames && s.hovered, "held open");
    e.frame(&mut s, t + ms(1200));
    e.frame(&mut s, t + ms(1200) + SETTLE);
    assert!(!e.is_open());
    assert!(!s.frames && !s.hovered, "at rest: no frames, no hook");
}

#[test]
fn throwing_the_only_card_leaves_nothing_running() {
    let mut s = Rec::new(1.0);
    let t0 = Instant::now();
    let (e, k, rx) = dismissable(1, &mut s, t0);
    let t = t0 + SETTLE;
    e.mouse(&mut s, k[0], Mouse::Move, 10, 10, t);
    assert!(e.is_open() && s.hovered);
    let start = s.at(k[0]);
    let t = drag(&e, &mut s, k[0], &[(15, 0), (30, 0)], ms(10), t);
    s.cursor = Some((start.0 + 90, start.1 + 20));
    assert!(release(&e, &mut s, k[0], 30, 0, start, t));
    assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), 0);
    e.sync(&mut s, t);
    e.frame(&mut s, t + SETTLE);
    e.frame(&mut s, t + SETTLE * 2);
    assert!(s.wins.is_empty(), "the window is gone");
    assert!(!e.is_open());
    assert!(!s.frames && !s.hovered && !s.timer);
}

#[test]
fn a_cancelled_drag_springs_back_without_a_dismiss() {
    let mut s = Rec::new(1.0);
    let t0 = Instant::now();
    let (e, k, rx) = dismissable(2, &mut s, t0);
    let t = t0 + SETTLE;
    e.mouse(&mut s, k[1], Mouse::Move, 10, 10, t);
    e.frame(&mut s, t + SETTLE);
    let t = t + SETTLE;
    let start = s.at(k[1]);
    let t = drag(&e, &mut s, k[1], &[(40, 0), (150, 0)], ms(200), t);
    // The capture is taken away mid-drag, past the throw line.
    assert!(!e.mouse(&mut s, k[1], Mouse::Cancel, 0, 0, t));
    e.frame(&mut s, t + SETTLE);
    assert_eq!(s.at(k[1]), start, "back in its slot");
    assert!(rx.try_recv().is_err(), "not thrown");
    assert_eq!(e.keys(), k);
    // The Up that follows a cancel is no click either.
    let (x, y) = s.local(k[1], start.0 + 60, start.1 + 20);
    assert!(!e.mouse(&mut s, k[1], Mouse::Up, x, y, t + SETTLE));
    // And the stack still folds when the cursor leaves.
    s.cursor = Some((0, 0));
    e.mouse(&mut s, k[1], Mouse::Leave, 0, 0, t + SETTLE);
    assert!(!e.is_open());
}

#[test]
fn an_up_then_its_capture_cancel_clicks_once() {
    let mut s = Rec::new(1.0);
    let t0 = Instant::now();
    let (e, k, rx) = dismissable(1, &mut s, t0);
    let t = t0 + SETTLE;
    e.mouse(&mut s, k[0], Mouse::Down, 60, 20, t);
    assert!(e.mouse(&mut s, k[0], Mouse::Up, 60, 20, t));
    // Win32's ReleaseCapture after the Up sends WM_CAPTURECHANGED.
    assert!(!e.mouse(&mut s, k[0], Mouse::Cancel, 0, 0, t));
    assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), 0);
    assert!(rx.recv_timeout(ms(200)).is_err(), "once");
}

#[test]
fn the_wheel_clamps_at_the_oldest_end_too() {
    let mut s = Rec::new(1.0);
    let t0 = Instant::now();
    let (e, k) = crowded(CORNER_BOTTOM_RIGHT, &mut s, t0);
    let mut t = t0 + SETTLE;
    e.mouse(&mut s, k[11], Mouse::Move, 10, 10, t);
    assert_eq!(e.scroll(), Some((8, 8)));
    for want in (0..8).rev() {
        t += ms(100);
        assert!(e.wheel(&mut s, Wheel::Down, t));
        assert_eq!(e.scroll(), Some((want, 8)));
    }
    assert!(
        !e.wheel(&mut s, Wheel::Down, t + ms(100)),
        "clamped at the oldest"
    );
    e.frame(&mut s, t + SETTLE);
    let folded: Vec<bool> = k.iter().map(|x| e.slot(*x).unwrap().1).collect();
    assert!(!folded[0], "the oldest in full, no corner deck");
    for x in &k {
        assert!(inside(rect_of(&s, *x), s.work), "on screen");
    }
}

#[test]
fn in_a_top_corner_dragging_down_goes_older() {
    let mut s = Rec::new(1.0);
    let t0 = Instant::now();
    let (e, k) = crowded(CORNER_TOP_RIGHT, &mut s, t0);
    let t = t0 + SETTLE;
    e.mouse(&mut s, k[11], Mouse::Move, 10, 10, t);
    e.frame(&mut s, t + SETTLE);
    let t = t + SETTLE;
    assert_eq!(e.scroll(), Some((8, 8)));
    // Down the screen in a top corner pulls the corner end into view.
    drag(&e, &mut s, k[10], &[(0, 13), (0, 50)], ms(50), t);
    assert_eq!(e.scroll(), Some((7, 8)));
}

#[test]
fn a_card_sliding_under_a_still_cursor_off_the_stack_does_not_open_it() {
    let mut s = Rec::new(1.0);
    let t0 = Instant::now();
    let (e, k) = stack(2, CORNER_BOTTOM_RIGHT, &mut s, t0);
    let t = t0 + SETTLE;
    // The cursor rests in the margin right of the stack.
    let r = rect_of(&s, k[1]);
    let (cx, cy) = (r.r + 10, r.b - 70);
    s.cursor = Some((cx, cy));
    e.post(
        Note {
            sticky: true,
            ..note("new")
        },
        t,
    );
    e.sync(&mut s, t);
    let n = *e.keys().last().unwrap();
    e.frame(&mut s, t + ms(30));
    let at = rect_of(&s, n);
    assert!(at.has(cx, cy), "the new card is under the cursor mid-slide");
    // Windows' move for a window passing under a still cursor.
    e.mouse(&mut s, n, Mouse::Move, cx - at.l, cy - at.t, t + ms(30));
    assert!(!e.is_open(), "off the stack: stays folded");
    assert!(!s.hovered);
}
