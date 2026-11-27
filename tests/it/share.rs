//! The shared stack (`toast::share`): two "programs" (each a `Share` over
//! an engine of its own) in one share folder, talking over loopback as two
//! real programs do. The host's engine is synced on a recording surface.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use toast::engine::{Engine, SHARED_MARK};
use toast::layout::Part;
use toast::share::{Msg, PostMsg, Share, read_msg};
use toast::{Action, CORNER_BOTTOM_LEFT, CORNER_BOTTOM_RIGHT, Config, Kind, Note, Options, Scheme};

use crate::engine::{Rec, rect_of};

fn noop() {}
fn no_sound(_: Arc<[u8]>) {}

/// A share folder of the test's own.
fn dir(name: &str) -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    let d = std::env::temp_dir().join(format!(
        "rust-toast-share-test-{}-{name}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&d);
    d
}

/// A program: its engine and its way into the shared stack.
fn program(app: &str, corner: &str, dir: &Path) -> (&'static Engine, &'static Share) {
    let e: &'static Engine = Box::leak(Box::new(Engine::new()));
    e.configure(Config {
        app_name: app.into(),
        ..Config::default()
    });
    let s: &'static Share = Box::leak(Box::new(Share::new(
        e,
        Some(dir.to_path_buf()),
        noop,
        no_sound,
    )));
    s.set_options(Options {
        corner: corner.into(),
        sound: false,
        ..Options::default()
    });
    (e, s)
}

fn sticky(title: &str) -> Note {
    Note {
        title: title.into(),
        sticky: true,
        ..Note::default()
    }
}

/// Waits up to 3 s for f.
fn soon(mut f: impl FnMut() -> bool) -> bool {
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(3) {
        if f() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    false
}

fn titles(e: &Engine) -> Vec<String> {
    e.notes().into_iter().map(|n| n.title).collect()
}

#[test]
fn messages_survive_the_wire() {
    let post = Msg::Post(Box::new(PostMsg {
        serial: 7,
        id: "build".into(),
        title: "Build finished".into(),
        body: "All tests passed.".into(),
        kind: Kind::Success,
        accent: Some(toast::CardAccent {
            light: toast::hex("#7c3aed"),
            dark: toast::hex("#a78bfa"),
        }),
        sticky: false,
        expire_ms: 2500,
        burst: true,
        clickable: true,
        actions: vec![("Open".into(), true), ("Later".into(), false)],
        image: Some((2, 1, vec![1, 2, 3, 4, 5, 6, 7, 8])),
    }));
    let look = Config {
        app_name: "Suzuri".into(),
        scheme: Scheme::Light,
        corner_radius: 10.0,
        glass: 0.6,
        stripe: false,
        capture_exclude: false,
        ..Config::default()
    }
    .normalized();
    let msgs = [
        Msg::Hello {
            wire: toast::share::WIRE,
            token: "t0k3n".into(),
            pid: 42,
            look: Box::new(look.clone()),
        },
        post,
        Msg::Close("build".into()),
        Msg::DismissAll,
        Msg::Restyle(Box::new(look)),
        Msg::Welcome,
        Msg::Action(7, 1),
        Msg::Click(7),
        Msg::Dismiss(7),
        Msg::Gone(7),
    ];
    let mut wire = vec![];
    for m in &msgs {
        wire.extend(m.frame());
    }
    let mut r = &wire[..];
    for m in &msgs {
        assert_eq!(&read_msg(&mut r).unwrap(), m);
    }
    assert!(read_msg(&mut r).is_err(), "nothing left");
    // A frame cut short, or with a byte too many, is not a message.
    let f = Msg::Gone(3).frame();
    assert_eq!(Msg::parse(&f[4..f.len() - 1]), None);
    let mut long = f[4..].to_vec();
    long.push(0);
    assert_eq!(Msg::parse(&long), None);
}

#[test]
fn two_programs_in_one_corner_share_one_stack() {
    let d = dir("one-stack");
    let (ea, a) = program("Hearth", CORNER_BOTTOM_RIGHT, &d);
    let (eb, b) = program("Suzuri", CORNER_BOTTOM_RIGHT, &d);
    a.post(sticky("Hearth: card landed"));
    assert!(a.is_host(), "the first card in a corner hosts it");
    b.configure(Config {
        app_name: "Suzuri".into(),
        scheme: Scheme::Light,
        corner_radius: 10.0,
        capture_exclude: false,
        ..Config::default()
    });
    b.post(sticky("Suzuri: build finished"));
    assert!(b.is_client(), "the second joins the first's stack");
    assert!(soon(|| ea.notes().len() == 2), "{:?}", titles(ea));
    assert_eq!(
        titles(ea),
        ["Hearth: card landed", "Suzuri: build finished"]
    );
    assert!(eb.notes().is_empty(), "the client draws nothing itself");
    assert!(b.visible(), "but knows its card is up");

    // One stack: the cards sit one past the other, never on each other.
    let mut s = Rec::new(1.0);
    let t0 = Instant::now();
    ea.sync(&mut s, t0);
    ea.frame(&mut s, t0 + Duration::from_secs(1));
    let k = ea.keys();
    assert_eq!(k.len(), 2);
    let (r0, r1) = (rect_of(&s, k[0]), rect_of(&s, k[1]));
    assert!(r1.b <= r0.t, "the newer card stacks above: {r0:?} {r1:?}");
    assert_eq!(r0.r, r1.r, "in one column");

    // Each card in its own program's look.
    let la = ea.look(k[0]).unwrap();
    let lb = ea.look(k[1]).unwrap();
    assert!(la.dark && !lb.dark, "Hearth's dark, Suzuri's light");
    assert_eq!(lb.radius, 10);
    assert!(s.capture_excluded(k[0]) && !s.capture_excluded(k[1]));
}

#[test]
fn the_persons_actions_reach_the_program_that_posted() {
    let d = dir("actions");
    let (ea, a) = program("Hearth", CORNER_BOTTOM_RIGHT, &d);
    let (_eb, b) = program("Suzuri", CORNER_BOTTOM_RIGHT, &d);
    a.post(sticky("host"));
    let (tx, rx) = mpsc::channel();
    let (t1, t2) = (tx.clone(), tx.clone());
    b.post(Note {
        id: "update".into(),
        title: "Suzuri 1.2 is ready".into(),
        sticky: true,
        actions: vec![
            Action::new("Restart", move || t1.send("restart").unwrap()),
            Action::close("Later"),
        ],
        on_dismiss: Some(Arc::new(move || t2.send("dismiss").unwrap())),
        ..Note::default()
    });
    assert!(soon(|| ea.notes().len() == 2));
    let n = &ea.notes()[1];
    assert!(n.id.starts_with(SHARED_MARK), "{:?}", n.id);
    assert!(n.silent, "the host never plays another program's sound");
    let mut s = Rec::new(1.0);
    ea.sync(&mut s, Instant::now());
    let key = ea.keys()[1];
    assert!(ea.click(key, Part::Button(0)));
    assert_eq!(rx.recv_timeout(Duration::from_secs(3)), Ok("restart"));
    assert!(soon(|| b.sent().is_empty()), "the card is gone for b too");

    // Clear all on the host's stack dismisses every program's cards.
    let tx3 = tx.clone();
    b.post(Note {
        title: "again".into(),
        sticky: true,
        on_dismiss: Some(Arc::new(move || tx3.send("dismiss").unwrap())),
        ..Note::default()
    });
    assert!(soon(|| ea.notes().len() == 2));
    assert!(ea.dismiss_all());
    assert_eq!(rx.recv_timeout(Duration::from_secs(3)), Ok("dismiss"));
    assert!(soon(|| b.sent().is_empty()));

    // A program's own dismiss_all and close touch only its own cards.
    a.post(sticky("host again"));
    b.post(Note {
        id: "b1".into(),
        ..sticky("b1")
    });
    b.post(sticky("b2"));
    assert!(soon(|| ea.notes().len() == 3));
    b.close("b1");
    assert!(
        soon(|| titles(ea) == ["host again", "b2"]),
        "{:?}",
        titles(ea)
    );
    a.dismiss_all();
    assert!(soon(|| titles(ea) == ["b2"]), "{:?}", titles(ea));
    b.dismiss_all();
    assert!(soon(|| ea.notes().is_empty()));
    assert!(soon(|| b.sent().is_empty()));
}

#[test]
fn a_card_that_expires_on_the_host_is_forgotten_by_its_program() {
    let d = dir("expire");
    let (ea, a) = program("Hearth", CORNER_BOTTOM_RIGHT, &d);
    let (_eb, b) = program("Suzuri", CORNER_BOTTOM_RIGHT, &d);
    a.post(sticky("host"));
    b.post(Note {
        title: "short".into(),
        expire: Duration::from_millis(50),
        ..Note::default()
    });
    assert!(soon(|| ea.notes().len() == 2));
    assert!(soon(|| {
        ea.tick(Instant::now());
        ea.notes().len() == 1
    }));
    assert!(soon(|| b.sent().is_empty()));
}

#[test]
fn when_the_host_leaves_the_next_program_takes_the_corner_with_its_cards() {
    let d = dir("handover");
    let (ea, a) = program("Hearth", CORNER_BOTTOM_RIGHT, &d);
    let (eb, b) = program("Suzuri", CORNER_BOTTOM_RIGHT, &d);
    let (ec, c) = program("Irori", CORNER_BOTTOM_RIGHT, &d);
    a.post(sticky("a"));
    b.post(sticky("b"));
    c.post(sticky("c"));
    assert!(soon(|| ea.notes().len() == 3), "{:?}", titles(ea));

    // The host's person moves its cards to another corner: it gives this
    // one up, and the others carry on in one stack without it.
    a.set_options(Options {
        corner: CORNER_BOTTOM_LEFT.into(),
        sound: false,
        ..Options::default()
    });
    assert_eq!(titles(ea), ["a"], "its own card moves with it");
    assert!(soon(|| b.is_host() || c.is_host()));
    let (eh, ch) = if b.is_host() { (eb, c) } else { (ec, b) };
    assert!(soon(|| eh.notes().len() == 2), "{:?}", titles(eh));
    let mut t = titles(eh);
    t.sort();
    assert_eq!(t, ["b", "c"]);
    assert!(ch.is_client());
}

#[test]
fn different_corners_are_different_stacks() {
    let d = dir("corners");
    let (ea, a) = program("Hearth", CORNER_BOTTOM_RIGHT, &d);
    let (eb, b) = program("Suzuri", CORNER_BOTTOM_LEFT, &d);
    a.post(sticky("a"));
    b.post(sticky("b"));
    assert!(a.is_host() && b.is_host());
    assert_eq!(titles(ea), ["a"]);
    assert_eq!(titles(eb), ["b"]);
}

#[test]
fn cards_off_on_a_client_takes_its_cards_down() {
    let d = dir("off");
    let (ea, a) = program("Hearth", CORNER_BOTTOM_RIGHT, &d);
    let (_eb, b) = program("Suzuri", CORNER_BOTTOM_RIGHT, &d);
    a.post(sticky("a"));
    b.post(sticky("b"));
    assert!(soon(|| ea.notes().len() == 2));
    b.set_options(Options {
        enabled: false,
        corner: CORNER_BOTTOM_RIGHT.into(),
        ..Options::default()
    });
    assert!(b.sent().is_empty());
    assert!(soon(|| titles(ea) == ["a"]), "{:?}", titles(ea));
    b.post(sticky("off"));
    assert!(b.sent().is_empty(), "off posts nothing");
}

#[test]
fn a_host_turning_cards_off_hands_the_others_cards_over() {
    let d = dir("host-off");
    let (ea, a) = program("Hearth", CORNER_BOTTOM_RIGHT, &d);
    let (eb, b) = program("Suzuri", CORNER_BOTTOM_RIGHT, &d);
    a.post(sticky("a"));
    b.post(sticky("b"));
    assert!(soon(|| ea.notes().len() == 2));
    a.set_options(Options {
        enabled: false,
        corner: CORNER_BOTTOM_RIGHT.into(),
        ..Options::default()
    });
    assert!(ea.notes().is_empty());
    assert!(soon(|| b.is_host()), "b takes the corner");
    assert!(soon(|| titles(eb) == ["b"]), "{:?}", titles(eb));
}
