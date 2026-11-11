//! The `rust-toast` program's protocol, headless on any OS: parsing, and
//! each command mapped onto the library through an engine driven with the
//! recording surface (the same one engine.rs uses), with the events it
//! reports.

use std::io::Cursor;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use toast::engine::Engine;
use toast::layout::Part;
use toast::note::FADE_OUT;
use toast::protocol::{
    self, Backend, Command, Emit, Event, Flow, How, KindName, MAX_BODY, MAX_ECHO, MAX_TITLE,
    Session,
};
use toast::{Config, Kind, Note, Options, Scheme, rgb};

use crate::engine::Rec;

/// The library's engine on a recording surface, at a clock the test moves.
struct Headless {
    e: Engine,
    rec: Mutex<Rec>,
    now: Mutex<Instant>,
    /// Options as the session last set them, in order (tests).
    sets: Mutex<Vec<Options>>,
    /// Posts and option changes wait for the next sync, as on a real
    /// platform (whose window thread syncs later).
    lazy: bool,
}

impl Headless {
    fn new() -> Headless {
        Headless {
            e: Engine::new(),
            rec: Mutex::new(Rec::new(1.0)),
            now: Mutex::new(Instant::now()),
            sets: Mutex::new(vec![]),
            lazy: false,
        }
    }

    fn now(&self) -> Instant {
        *self.now.lock().unwrap()
    }

    fn sync(&self) {
        self.e.sync(&mut *self.rec.lock().unwrap(), self.now());
    }

    /// Moves the clock on and runs the expiry tick, as the platform's
    /// timer does.
    fn advance(&self, d: Duration) {
        *self.now.lock().unwrap() += d;
        self.e.tick(self.now());
        self.sync();
    }

    /// The person clicks part p of the card with id.
    fn click(&self, id: &str, p: Part) {
        let key = self.key(id).expect("card is up");
        assert!(self.e.click(key, p));
        self.sync();
    }

    fn key(&self, id: &str) -> Option<u64> {
        let notes = self.e.notes();
        let i = notes.iter().position(|n| n.id == id)?;
        self.e.keys().get(i).copied()
    }

    fn note(&self, id: &str) -> Note {
        self.e
            .notes()
            .into_iter()
            .find(|n| n.id == id)
            .expect("note is up")
    }
}

impl Backend for &Headless {
    fn supported(&self) -> bool {
        true
    }
    fn configure(&self, c: Config) {
        self.e.configure(c);
        self.sync();
    }
    fn set_options(&self, o: Options) {
        self.sets.lock().unwrap().push(o.clone());
        self.e.set_options(o);
        if !self.lazy {
            self.sync();
        }
    }
    fn options(&self) -> Options {
        self.e.options()
    }
    fn post(&self, n: Note) {
        self.e.post(n, self.now());
        if !self.lazy {
            self.sync();
        }
    }
    fn close(&self, id: &str) {
        self.e.close(id);
        self.sync();
    }
}

/// A backend for a system without cards.
struct Unsupported;

impl Backend for Unsupported {
    fn supported(&self) -> bool {
        false
    }
    fn configure(&self, _: Config) {
        panic!("no-op system got configure")
    }
    fn set_options(&self, _: Options) {
        panic!("no-op system got set_options")
    }
    fn options(&self) -> Options {
        Options::default()
    }
    fn post(&self, _: Note) {
        panic!("no-op system got a post")
    }
    fn close(&self, _: &str) {
        panic!("no-op system got close")
    }
}

fn events() -> (Emit, mpsc::Receiver<Event>) {
    let (tx, rx) = mpsc::channel();
    let tx = Mutex::new(tx);
    (
        Arc::new(move |e| {
            let _ = tx.lock().unwrap().send(e);
        }),
        rx,
    )
}

fn drain(rx: &mpsc::Receiver<Event>) -> Vec<Event> {
    rx.try_iter().collect()
}

/// The next event, waiting for a callback thread.
fn next(rx: &mpsc::Receiver<Event>) -> Event {
    rx.recv_timeout(Duration::from_secs(2))
        .expect("an event within 2 s")
}

fn session(h: &Headless) -> (Session<&Headless>, mpsc::Receiver<Event>) {
    let (emit, rx) = events();
    let s = Session::new(h, Config::default(), emit);
    (s, rx)
}

fn dismiss(id: &str, how: How) -> Event {
    Event::Dismiss { id: id.into(), how }
}

fn posted(id: &str) -> Event {
    Event::Posted { id: id.into() }
}

#[test]
fn parses_every_command_and_ignores_unknown_fields() {
    let c = protocol::parse(
        br#"{"cmd":"post","id":"a","title":"T","body":"B","kind":"warning","sticky":true,
            "timeout_ms":1500,"thumbnail":"/x.png","sound":false,"confetti":true,
            "actions":[{"id":"open","label":"Open","colour":"red"},{"id":"later","label":"Later"}],
            "future_field":{"nested":[1,2]}}"#,
    )
    .unwrap();
    let Command::Post(p) = c else {
        panic!("not a post: {c:?}")
    };
    assert_eq!(p.id.as_deref(), Some("a"));
    assert_eq!(p.kind, Some(KindName::Warn), "warning is an alias of warn");
    assert!(p.sticky);
    assert_eq!(p.timeout_ms, Some(1500));
    assert_eq!(p.thumbnail.as_deref(), Some("/x.png"));
    assert_eq!((p.sound, p.confetti), (Some(false), Some(true)));
    assert_eq!(p.actions.len(), 2);
    assert_eq!(
        (p.actions[0].id.as_str(), p.actions[0].label.as_str()),
        ("open", "Open")
    );

    assert_eq!(
        protocol::parse(br#"{"cmd":"close","id":"a","why":1}"#).unwrap(),
        Command::Close { id: "a".into() }
    );
    assert_eq!(
        protocol::parse(br#"{"cmd":"quit"}"#).unwrap(),
        Command::Quit
    );
    assert_eq!(
        protocol::parse(br#"{"cmd":"list"}"#).unwrap(),
        Command::List
    );
    assert!(matches!(
        protocol::parse(br#"{"cmd":"configure","app_name":"Hearth","scheme":"system"}"#),
        Ok(Command::Configure(_))
    ));
    assert!(matches!(
        protocol::parse(br#"{"cmd":"set_options","corner":"bottom-left","volume":0.3}"#),
        Ok(Command::SetOptions(_))
    ));
}

#[test]
fn malformed_lines_are_errors_not_panics() {
    for bad in [
        &b"not json"[..],
        b"{",
        b"[]",
        b"42",
        b"{}",
        br#"{"cmd":"explode"}"#,
        br#"{"cmd":"post","title":5}"#,
        br#"{"cmd":"post","kind":"purple"}"#,
        br#"{"cmd":"close"}"#,
        b"\xff\xfe\x00garbage",
    ] {
        assert!(protocol::parse(bad).is_err(), "{bad:?} parsed");
    }
    let h = Headless::new();
    let (mut s, rx) = session(&h);
    assert_eq!(s.line(b"\x00\x01{nope\x1b[2J"), Flow::Go);
    let e = drain(&rx);
    assert_eq!(e.len(), 1, "{e:?}");
    let Event::Error { command, .. } = &e[0] else {
        panic!("{e:?}")
    };
    assert_eq!(
        command.as_deref(),
        Some("{nope[2J"),
        "control characters dropped"
    );
    assert_eq!(s.line(b"   "), Flow::Go, "a blank line is nothing");
    assert!(drain(&rx).is_empty());
    assert_eq!(s.line(br#"{"cmd":"quit"}"#), Flow::Quit);
}

#[test]
fn echo_and_strings_are_bounded() {
    let long = "x".repeat(10_000);
    let e = protocol::echo(long.as_bytes());
    assert_eq!(e.chars().count(), MAX_ECHO + 1, "cut, with an ellipsis");

    let h = Headless::new();
    let (mut s, rx) = session(&h);
    let line = format!(
        r#"{{"cmd":"post","id":"{long}","title":"{long}","body":"{long}","actions":[{{"id":"{long}","label":"{long}"}}]}}"#
    );
    s.line(line.as_bytes());
    let Event::Posted { id } = next(&rx) else {
        panic!("not posted")
    };
    assert_eq!(id.chars().count(), protocol::MAX_ID);
    let n = h.note(&id);
    assert_eq!(n.title.chars().count(), MAX_TITLE);
    assert_eq!(n.body.chars().count(), MAX_BODY);
    assert_eq!(n.actions[0].label.chars().count(), protocol::MAX_LABEL);
}

#[test]
fn read_line_bounds_lines_and_reads_on() {
    let mut r = Cursor::new(b"short\r\n0123456789abcdef\nnext\nlast".to_vec());
    let mut read = || protocol::read_line(&mut r, 10).unwrap();
    assert_eq!(read(), Some(Ok(b"short".to_vec())), "CRLF ok");
    assert_eq!(read(), Some(Err(b"0123456789".to_vec())), "too long");
    assert_eq!(read(), Some(Ok(b"next".to_vec())), "the next line is whole");
    assert_eq!(read(), Some(Ok(b"last".to_vec())), "no newline at the end");
    assert_eq!(read(), None, "end of input");
}

#[test]
fn post_maps_onto_the_library() {
    let h = Headless::new();
    let (mut s, rx) = session(&h);
    s.line(
        br#"{"cmd":"post","id":"ask","title":" Waiting ","body":"Open it","kind":"error",
            "sticky":true,"sound":false,
            "actions":[{"id":"open","label":"Open"},{"id":"later","label":"Later"},{"id":"x","label":"Dropped"}]}"#,
    );
    assert_eq!(drain(&rx), vec![posted("ask")]);
    let n = h.note("ask");
    assert_eq!(n.title, "Waiting");
    assert_eq!(n.body, "Open it");
    assert_eq!(n.kind, Kind::Error);
    assert!(n.sticky && n.silent);
    assert!(n.on_click.is_some() && n.on_dismiss.is_some());
    let look = h.e.look(h.key("ask").unwrap()).unwrap();
    assert_eq!(look.labels, vec!["Open", "Later"], "two buttons at most");
    assert_eq!(look.kind, Kind::Error);
    assert!(look.clickable);

    // Defaults: info, the library's lifetime, an id made up.
    s.line(br#"{"cmd":"post","body":"just a body"}"#);
    assert_eq!(drain(&rx), vec![posted("auto-1")]);
    let n = h.note("auto-1");
    assert_eq!(
        (n.title.as_str(), n.kind, n.sticky),
        ("just a body", Kind::Info, false)
    );
    assert_eq!(n.expire, Duration::ZERO);

    // An action with no id reports its index; no label shows the id.
    s.line(br#"{"cmd":"post","id":"b","title":"t","clickable":false,"actions":[{"label":"Go"},{"id":"skip"}]}"#);
    let look = h.e.look(h.key("b").unwrap()).unwrap();
    assert_eq!(look.labels, vec!["Go", "skip"]);
    assert!(!look.clickable, "clickable false: a body click dismisses");

    s.line(br#"{"cmd":"post","title":"","body":"  "}"#);
    assert!(matches!(&drain(&rx)[..], [_, Event::Error { .. }]));
}

#[test]
fn buttons_and_clicks_report_once() {
    let h = Headless::new();
    let (mut s, rx) = session(&h);
    s.line(br#"{"cmd":"post","id":"a","title":"A","actions":[{"id":"open","label":"Open"},{"id":"later","label":"Later"}]}"#);
    s.line(br#"{"cmd":"post","id":"b","title":"B"}"#);
    s.line(br#"{"cmd":"post","id":"c","title":"C"}"#);
    s.line(br#"{"cmd":"post","id":"d","title":"D","clickable":false}"#);
    drain(&rx);

    h.click("a", Part::Button(1));
    assert_eq!(
        next(&rx),
        Event::Action {
            id: "a".into(),
            action: "later".into()
        }
    );
    h.click("b", Part::Body);
    assert_eq!(next(&rx), Event::Click { id: "b".into() });
    h.click("c", Part::Close);
    assert_eq!(next(&rx), dismiss("c", How::User));
    h.click("d", Part::Body);
    assert_eq!(
        next(&rx),
        dismiss("d", How::User),
        "not clickable: a dismiss"
    );

    std::thread::sleep(Duration::from_millis(200));
    assert!(drain(&rx).is_empty(), "one event a card, no timeout after");
    assert!(s.ids().is_empty());
}

#[test]
fn cards_going_by_themselves_say_how() {
    let h = Headless::new();
    let (mut s, rx) = session(&h);

    // Expiry.
    s.line(br#"{"cmd":"post","id":"t","title":"T","timeout_ms":500}"#);
    assert_eq!(h.note("t").expire, Duration::from_millis(500));
    h.advance(Duration::from_millis(400));
    s.line(br#"{"cmd":"list"}"#);
    assert_eq!(
        drain(&rx),
        vec![
            posted("t"),
            Event::Cards {
                ids: vec!["t".into()]
            }
        ]
    );
    h.advance(Duration::from_millis(150)); // fading
    assert!(drain(&rx).is_empty());
    h.advance(FADE_OUT);
    assert_eq!(drain(&rx), vec![dismiss("t", How::Timeout)]);

    // close.
    s.line(br#"{"cmd":"post","id":"c","title":"C","sticky":true}"#);
    s.line(br#"{"cmd":"close","id":"c"}"#);
    s.line(br#"{"cmd":"close","id":"nothing-here"}"#);
    assert_eq!(drain(&rx), vec![posted("c"), dismiss("c", How::Closed)]);

    // A replace in place reports nothing: the id lives on, with its
    // place in the list.
    s.line(br#"{"cmd":"post","id":"r","title":"one","sticky":true}"#);
    s.line(br#"{"cmd":"post","id":"q","title":"two","sticky":true}"#);
    s.line(br#"{"cmd":"post","id":"r","title":"one again","sticky":true}"#);
    assert_eq!(drain(&rx), vec![posted("r"), posted("q"), posted("r")]);
    assert_eq!(h.note("r").title, "one again");
    assert_eq!(s.ids(), vec!["r", "q"]);

    // Turning cards off takes them all down.
    s.line(br#"{"cmd":"set_options","enabled":false}"#);
    let mut e = drain(&rx);
    e.sort_by_key(|e| format!("{e:?}"));
    assert_eq!(
        e,
        vec![dismiss("q", How::Closed), dismiss("r", How::Closed)]
    );
    s.line(br#"{"cmd":"post","id":"off","title":"nobody sees this"}"#);
    assert!(matches!(&drain(&rx)[..], [Event::Error { id: Some(i), .. }] if i == "off"));
}

#[test]
fn a_full_stack_pushes_the_oldest_off() {
    let h = Headless::new();
    let (mut s, rx) = session(&h);
    s.line(br#"{"cmd":"configure","max_cards":2}"#);
    assert_eq!(h.e.config().motion.max_cards, 2);
    for id in ["a", "b", "c"] {
        s.line(format!(r#"{{"cmd":"post","id":"{id}","title":"{id}"}}"#).as_bytes());
    }
    assert_eq!(
        drain(&rx),
        vec![
            posted("a"),
            posted("b"),
            dismiss("a", How::Pushed),
            posted("c")
        ]
    );
    assert_eq!(s.ids(), vec!["b", "c"]);
}

#[test]
fn configure_and_options_merge_and_validate() {
    let h = Headless::new();
    let (mut s, rx) = session(&h);
    s.line(
        br##"{"cmd":"configure","app_name":"Hearth","scheme":"light","corner_radius":10,
            "glass":0.6,"stripe":false,"capture_exclude":false,"shown":2,
            "light":{"background":"#fafafa","accent":{"info":"112233"}}}"##,
    );
    s.line(br#"{"cmd":"configure","corner_radius":12}"#);
    assert!(drain(&rx).is_empty());
    let c = h.e.config();
    assert_eq!(c.app_name, "Hearth");
    assert_eq!(c.scheme, Scheme::Light);
    assert_eq!(c.corner_radius, 12.0, "a later configure keeps the rest");
    assert_eq!(c.glass, 0.6);
    assert!(!c.stripe && !c.capture_exclude);
    assert_eq!(c.motion.shown, 2);
    assert_eq!(c.light.background, rgb(0xfa, 0xfa, 0xfa));
    assert_eq!(c.light.accent.info, rgb(0x11, 0x22, 0x33));
    assert_eq!(
        c.light.primary,
        rgb(0x11, 0x22, 0x33),
        "derived colours follow"
    );

    s.line(br##"{"cmd":"configure","app_name":"Nope","dark":{"muted":"#12345"}}"##);
    assert!(matches!(&drain(&rx)[..], [Event::Error { .. }]));
    assert_eq!(
        h.e.config().app_name,
        "Hearth",
        "a bad configure changes nothing"
    );

    s.line(br#"{"cmd":"set_options","corner":"bottom-left","volume":3,"sound":false}"#);
    s.line(br#"{"cmd":"set_options","confetti":true}"#);
    let o = h.e.options();
    assert_eq!(o.corner, toast::CORNER_BOTTOM_LEFT);
    assert_eq!(o.volume, 1.0, "clamped");
    assert!(!o.sound && o.confetti && o.enabled);
    s.line(br#"{"cmd":"set_options","corner":"centre"}"#);
    s.line(br#"{"cmd":"set_options","volume":null,"enabled":"yes"}"#);
    assert_eq!(drain(&rx).len(), 2);
    assert_eq!(h.e.options().corner, toast::CORNER_BOTTOM_LEFT);
}

#[test]
fn per_card_confetti_leaves_the_options_as_they_were() {
    let h = Headless::new();
    let (mut s, rx) = session(&h);
    s.line(br#"{"cmd":"post","id":"party","title":"Shipped","kind":"success","confetti":true}"#);
    assert_eq!(drain(&rx), vec![posted("party")]);
    assert_eq!(
        h.rec.lock().unwrap().bursts.len(),
        1,
        "a burst for this card"
    );
    assert!(!h.e.options().confetti, "options back as they were");
    let sets = h.sets.lock().unwrap().clone();
    assert_eq!(
        sets.iter().map(|o| o.confetti).collect::<Vec<_>>(),
        vec![true, false]
    );

    s.line(br#"{"cmd":"post","title":"Plain","kind":"success"}"#);
    assert_eq!(h.rec.lock().unwrap().bursts.len(), 1, "no burst by default");

    // The real window thread syncs after the options are put back: the
    // burst is decided at the post.
    let h = Headless {
        lazy: true,
        ..Headless::new()
    };
    let (mut s, _rx) = session(&h);
    s.line(br#"{"cmd":"post","id":"party","title":"Shipped","kind":"success","confetti":true}"#);
    assert!(!h.e.options().confetti);
    h.sync();
    assert_eq!(
        h.rec.lock().unwrap().bursts.len(),
        1,
        "a burst after a late sync"
    );
}

#[test]
fn an_unsupported_system_takes_commands_as_no_ops() {
    let (emit, rx) = events();
    let mut s = Session::new(Unsupported, Config::default(), emit);
    let Event::Ready {
        supported,
        protocol,
        version,
        ..
    } = s.ready()
    else {
        panic!()
    };
    assert!(!supported);
    assert_eq!(protocol, protocol::PROTOCOL);
    assert_eq!(version, env!("CARGO_PKG_VERSION"));
    for l in [
        &br#"{"cmd":"configure","app_name":"x"}"#[..],
        br#"{"cmd":"set_options","corner":"top-left"}"#,
        br#"{"cmd":"post","id":"n","title":"t"}"#,
        br#"{"cmd":"close","id":"n"}"#,
    ] {
        assert_eq!(s.line(l), Flow::Go);
    }
    assert_eq!(drain(&rx), vec![posted("n")]);
    s.line(br#"{"cmd":"set_options","corner":"nowhere"}"#);
    assert!(
        matches!(&drain(&rx)[..], [Event::Error { .. }]),
        "still validated"
    );
}

#[test]
fn events_are_one_json_line_each() {
    let lines = [
        Event::Ready {
            version: "0.3.0".into(),
            protocol: 1,
            platform: "macos".into(),
            supported: true,
        }
        .to_line(),
        Event::Action {
            id: "a".into(),
            action: "open".into(),
        }
        .to_line(),
        dismiss("a", How::Timeout).to_line(),
        Event::error("bad\ncommand", Some(b"x\ny")).to_line(),
    ];
    assert_eq!(
        lines[0],
        r#"{"event":"ready","version":"0.3.0","protocol":1,"platform":"macos","supported":true}"#
    );
    assert_eq!(lines[1], r#"{"event":"action","id":"a","action":"open"}"#);
    assert_eq!(lines[2], r#"{"event":"dismiss","id":"a","how":"timeout"}"#);
    assert!(!lines.iter().any(|l| l.contains('\n')));
}

mod thumbnails {
    use std::path::PathBuf;

    use toast::protocol::{load_thumbnail, shrink};

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rust-toast-test-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d.join(name)
    }

    fn write_png(path: &PathBuf, w: u32, h: u32, colour: png::ColorType) {
        let f = std::fs::File::create(path).unwrap();
        let mut e = png::Encoder::new(std::io::BufWriter::new(f), w, h);
        e.set_color(colour);
        e.set_depth(png::BitDepth::Eight);
        let n = match colour {
            png::ColorType::Rgb => 3,
            png::ColorType::Grayscale => 1,
            _ => 4,
        };
        let data: Vec<u8> = (0..w * h * n).map(|i| (i % 251) as u8).collect();
        e.write_header().unwrap().write_image_data(&data).unwrap();
    }

    #[test]
    fn pngs_load_and_big_ones_shrink() {
        let p = scratch("small.png");
        write_png(&p, 40, 20, png::ColorType::Rgba);
        let i = load_thumbnail(&p).unwrap();
        assert_eq!((i.w, i.h, i.rgba.len()), (40, 20, 40 * 20 * 4));

        let p = scratch("grey.png");
        write_png(&p, 30, 10, png::ColorType::Grayscale);
        let i = load_thumbnail(&p).unwrap();
        assert_eq!(&i.rgba[..4], &[0, 0, 0, 255]);

        let p = scratch("big.png");
        write_png(&p, 4000, 1000, png::ColorType::Rgb);
        let i = load_thumbnail(&p).unwrap();
        assert!(i.w <= 1144 && i.h <= 480, "shrunk to {}x{}", i.w, i.h);
        assert_eq!(i.rgba.len() as u32, i.w * i.h * 4);
    }

    #[test]
    fn anything_but_an_image_is_refused() {
        let p = scratch("secret.txt");
        std::fs::write(&p, "password=hunter2\n").unwrap();
        let e = load_thumbnail(&p).unwrap_err();
        assert_eq!(e, "thumbnail: not a PNG or JPEG");
        assert!(!e.contains("hunter2"));

        let p = scratch("truncated.png");
        std::fs::write(&p, b"\x89PNG\r\n\x1a\nnot really").unwrap();
        assert!(load_thumbnail(&p).is_err());

        let p = scratch("bad.jpg");
        std::fs::write(&p, [0xFF, 0xD8, 0xFF, 0xE0, 0, 0, 1, 2]).unwrap();
        assert!(load_thumbnail(&p).is_err());

        let d = scratch("a-dir");
        std::fs::create_dir_all(&d).unwrap();
        assert_eq!(
            load_thumbnail(&d).unwrap_err(),
            "thumbnail: not a regular file"
        );
        assert!(load_thumbnail(&scratch("missing.png")).is_err());
    }

    #[test]
    fn shrink_averages_and_keeps_small_images() {
        let (w, h, px) = shrink(2, 1, vec![0, 0, 0, 255, 255, 255, 255, 255]);
        assert_eq!((w, h, px.len()), (2, 1, 8), "small: untouched");
        let big = vec![200u8; 3000 * 3000 * 4];
        let (w, h, px) = shrink(3000, 3000, big);
        assert_eq!((w, h), (480, 480));
        assert!(px.iter().all(|&v| v == 200));
    }
}

#[test]
fn a_leading_byte_order_mark_is_skipped() {
    assert_eq!(
        protocol::parse(b"\xEF\xBB\xBF{\"cmd\":\"list\"}").unwrap(),
        Command::List
    );
    // Only at the start of a line.
    assert!(protocol::parse(b"{\"cmd\":\"list\"}\xEF\xBB\xBF").is_err());
}
