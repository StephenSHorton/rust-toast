//! `rust-toast`: the cards for programs not written in Rust (PROTOCOL.md).
//!
//! `rust-toast serve` reads newline-delimited JSON commands on stdin and
//! writes newline-delimited JSON events on stdout (logs on stderr). It
//! exits when stdin closes, so it never outlives the program that spawned
//! it, and on `{"cmd":"quit"}`, also when nobody reads its stdout.
//!
//! `rust-toast --title T [--body B ...]` shows one card, waits until it is
//! gone and prints how it went, as one event line. With `--with-parent` it
//! also ends (the card closed) when its stdin closes.
//!
//! On macOS the main thread runs NSApp (Accessory: no Dock icon, never
//! active) and the commands are read on another thread; on Windows the
//! library runs its own card thread. Elsewhere it reports
//! `"supported": false` and takes commands as no-ops.

use std::cell::Cell;
use std::collections::VecDeque;
use std::io::{self, Write};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use toast::protocol::{
    self, ActionCmd, Emit, Event, Flow, Global, KindName, MAX_LINE, PostCmd, Session,
};
use toast::{CORNER_BOTTOM_LEFT, CORNER_BOTTOM_RIGHT, CORNER_TOP_LEFT, CORNER_TOP_RIGHT, Config};

const USAGE: &str = "\
usage: rust-toast serve
       rust-toast --title TEXT [--body TEXT] [--kind info|success|warn|error]
                  [--sticky] [--timeout SECONDS] [--silent]
                  [--action ID:LABEL]... [--thumbnail PATH]
                  [--corner top-right|bottom-right|top-left|bottom-left]
                  [--app-name NAME] [--with-parent]
       rust-toast --version | --help";

const HELP: &str = "\
Native notification cards for Windows and macOS.

  serve              Long-lived: JSON commands on stdin, one per line; JSON
                     events on stdout, one per line. Exits when stdin
                     closes or on {\"cmd\":\"quit\"}. See PROTOCOL.md.

One card (exits once it is gone and prints how it went as one JSON line:
an action, click or dismiss event):

  --title TEXT       The card's title (or --body alone).
  --body TEXT        Up to two lines; longer text is ellipsized.
  --kind KIND        info (default), success, warn or error.
  --sticky           Stays until clicked or dismissed.
  --timeout SECONDS  How long it stays (default: 3 s, 6 s for warn and
                     error, 10 s with buttons). Ignored with --sticky.
  --silent           No sound.
  --action ID:LABEL  A button (at most two; the first is primary).
  --thumbnail PATH   A PNG or JPEG shown under the body.
  --corner CORNER    top-right (default), bottom-right, top-left,
                     bottom-left.
  --app-name NAME    The windows' title.
  --with-parent      Also ends, the card closed, when stdin closes: spawn
                     it with stdin piped, and it goes when your program
                     does (even if killed). Without it the card outlives
                     the program that started it.

  --version          Prints the version.
  --help             Prints this.

Exit status: 0 when the card went (or serve ended), 1 when it could not be
shown, 2 for bad arguments.";

#[derive(Debug, Default)]
struct Once {
    post: PostCmd,
    corner: Option<String>,
    app_name: Option<String>,
    with_parent: bool,
}

enum Mode {
    Serve,
    Once(Box<Once>),
}

fn parse_args(args: &[String]) -> Result<Option<Mode>, String> {
    let Some(first) = args.first() else {
        return Err("nothing to do".into());
    };
    match first.as_str() {
        "--help" | "-h" | "help" => {
            println!("{USAGE}\n\n{HELP}");
            return Ok(None);
        }
        "--version" | "-V" => {
            println!("rust-toast {}", env!("CARGO_PKG_VERSION"));
            return Ok(None);
        }
        "serve" if args.len() == 1 => return Ok(Some(Mode::Serve)),
        "serve" => return Err("serve takes no arguments".into()),
        _ => {}
    }
    let mut o = Once::default();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = |name: &str| {
            it.next()
                .cloned()
                .ok_or_else(|| format!("{name} needs a value"))
        };
        match a.as_str() {
            "--title" => o.post.title = val(a)?,
            "--body" => o.post.body = val(a)?,
            "--kind" => {
                o.post.kind = Some(match val(a)?.as_str() {
                    "info" => KindName::Info,
                    "success" => KindName::Success,
                    "warn" | "warning" => KindName::Warn,
                    "error" => KindName::Error,
                    k => return Err(format!("--kind: unknown kind {k:?}")),
                })
            }
            "--sticky" => o.post.sticky = true,
            "--silent" => o.post.sound = Some(false),
            "--timeout" => {
                let v = val(a)?;
                let s: f64 = v
                    .parse()
                    .ok()
                    .filter(|s: &f64| s.is_finite() && *s >= 0.0)
                    .ok_or_else(|| format!("--timeout: not a number of seconds: {v:?}"))?;
                o.post.timeout_ms = Some((s * 1000.0).round().min(u64::MAX as f64) as u64);
            }
            "--action" => {
                let v = val(a)?;
                let (id, label) = v.split_once(':').unwrap_or((&v, &v));
                if o.post.actions.len() == 2 {
                    return Err("--action: at most two".into());
                }
                o.post.actions.push(ActionCmd {
                    id: id.into(),
                    label: label.into(),
                });
            }
            "--thumbnail" => o.post.thumbnail = Some(val(a)?),
            "--corner" => {
                let c = val(a)?;
                if ![
                    CORNER_TOP_RIGHT,
                    CORNER_BOTTOM_RIGHT,
                    CORNER_TOP_LEFT,
                    CORNER_BOTTOM_LEFT,
                ]
                .contains(&c.as_str())
                {
                    return Err(format!(
                        "--corner: one of top-right, bottom-right, top-left, bottom-left, not {c:?}"
                    ));
                }
                o.corner = Some(c);
            }
            "--app-name" => o.app_name = Some(val(a)?),
            "--with-parent" => o.with_parent = true,
            _ => return Err(format!("unknown argument {a:?}")),
        }
    }
    if o.post.title.trim().is_empty() && o.post.body.trim().is_empty() {
        return Err("--title or --body is needed".into());
    }
    Ok(Some(Mode::Once(Box::new(o))))
}

fn main() {
    // A panic on any thread ends the program, rather than leaving the
    // AppKit loop running with nobody reading stdin.
    let report = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        report(info);
        std::process::exit(101);
    }));
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mode = match parse_args(&args) {
        Ok(Some(m)) => m,
        Ok(None) => return,
        Err(e) => {
            eprintln!("rust-toast: {e}\n{USAGE}");
            std::process::exit(2);
        }
    };
    platform::run(move || match mode {
        Mode::Serve => serve(),
        Mode::Once(o) => once(*o),
    });
}

/// What the writer thread is told.
enum Msg {
    Line(String),
    Exit(i32),
    /// Answer once everything before it is written.
    Flushed(mpsc::Sender<()>),
}

/// Event lines waiting for the writer, at most. The input thread waits for
/// room ([`STALL`]); the card threads never wait and drop past this.
const QUEUE: usize = 4096;

/// Of [`QUEUE`], the lines only the card threads may fill: while the input
/// thread keeps the queue full (a burst of commands), a card's event still
/// finds room.
const RESERVE: usize = 256;

/// How long one write to stdout may stay blocked before the reader counts
/// as stalled. While it is, events that find the queue full are dropped
/// (and counted) instead of waited for, so quit and the end of stdin still
/// end the program (PROTOCOL.md).
const STALL: Duration = Duration::from_millis(250);

/// How long `quit` or the end of stdin waits for the events before it
/// to be written.
const GRACE: Duration = Duration::from_millis(500);

thread_local! {
    /// Set on the thread that reads stdin: only it waits for room.
    static WAITS: Cell<bool> = const { Cell::new(false) };
}

#[derive(Default)]
struct State {
    q: VecDeque<Msg>,
    /// `Msg::Line`s in `q`.
    lines: usize,
    /// Events dropped and not reported yet.
    dropped: u64,
    /// When the write in progress started; `None` between writes.
    writing_since: Option<Instant>,
    /// The writer has ended (stdout's reader is gone).
    gone: bool,
}

impl State {
    /// Whether one write has been blocked for at least `d`.
    fn blocked_for(&self, d: Duration) -> bool {
        self.writing_since.is_some_and(|t| t.elapsed() >= d)
    }

    fn push(&mut self, m: Msg) {
        if matches!(m, Msg::Line(_)) {
            self.lines += 1;
        }
        self.q.push_back(m);
    }

    /// A line goes in: first the report of any dropped before it, so the
    /// report stands where the events went missing.
    fn push_line(&mut self, l: String) {
        if self.dropped > 0 {
            let report = dropped_report(std::mem::take(&mut self.dropped));
            self.push(Msg::Line(report));
        }
        self.push(Msg::Line(l));
    }
}

fn dropped_report(n: u64) -> String {
    let s = if n == 1 { "" } else { "s" };
    Event::error(format!("{n} event{s} dropped: stdout was not read"), None).to_line()
}

struct Queue {
    state: Mutex<State>,
    /// Something to write.
    ready: Condvar,
    /// The writer took a line, or ended.
    room: Condvar,
}

impl Queue {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// The writer thread's queue. The card threads never wait on it; the input
/// thread waits for room while the writer is moving (back-pressure on a
/// burst of commands) and drops once it has been stuck for [`STALL`].
#[derive(Clone)]
struct Out(Arc<Queue>);

impl Out {
    fn line(&self, l: String) {
        if WAITS.with(|w| w.get()) {
            self.line_waiting(l)
        } else {
            self.line_now(l)
        }
    }

    /// Never waits: past [`QUEUE`] the line is dropped and counted.
    fn line_now(&self, l: String) {
        let mut s = self.0.lock();
        if s.gone || s.lines >= QUEUE {
            s.dropped += 1;
            return;
        }
        s.push_line(l);
        drop(s);
        self.0.ready.notify_one();
    }

    /// Waits for room below [`QUEUE`] - [`RESERVE`] while the writer is
    /// moving; once one write has been blocked for [`STALL`], drops and
    /// counts the line at once, until a write gets through again.
    fn line_waiting(&self, l: String) {
        let mut s = self.0.lock();
        loop {
            if s.gone {
                s.dropped += 1;
                return;
            }
            if s.lines < QUEUE - RESERVE {
                break;
            }
            let left = match s.writing_since {
                Some(t) => STALL.saturating_sub(t.elapsed()),
                None => STALL,
            };
            if left.is_zero() {
                s.dropped += 1;
                return;
            }
            s = self
                .0
                .room
                .wait_timeout(s, left)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        s.push_line(l);
        drop(s);
        self.0.ready.notify_one();
    }

    /// The writer exits with code once the lines before are written.
    fn exit_after(&self, code: i32) {
        self.0.lock().push(Msg::Exit(code));
        self.0.ready.notify_one();
    }

    /// Exits with code once the queue is written, or after [`GRACE`] if
    /// stdout isn't being read (sooner if a write has already been stuck
    /// that long). The exit doesn't wait behind a write.
    fn exit_soon(&self, code: i32) -> ! {
        let end = Instant::now() + GRACE;
        let (done, written) = mpsc::channel();
        self.0.lock().push(Msg::Flushed(done));
        self.0.ready.notify_one();
        loop {
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            match written.recv_timeout(left.min(Duration::from_millis(10))) {
                Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    let s = self.0.lock();
                    if s.gone || s.blocked_for(GRACE) {
                        break;
                    }
                }
            }
        }
        platform::exit(code);
        // On macOS the exit runs between AppKit's events; should that loop
        // be stuck, end anyway.
        std::thread::sleep(Duration::from_secs(2));
        std::process::exit(code)
    }
}

/// Events go through one writer thread: a line at a time, flushed, so the
/// card threads never wait on a slow reader. A write that fails means the
/// reader is gone: the program exits.
fn writer() -> Out {
    let q = Arc::new(Queue {
        state: Mutex::new(State::default()),
        ready: Condvar::new(),
        room: Condvar::new(),
    });
    let shared = q.clone();
    std::thread::Builder::new()
        .name("rust-toast-out".into())
        .spawn(move || {
            let q = shared;
            let mut out = io::stdout().lock();
            loop {
                let m = {
                    let mut s = q.lock();
                    s.writing_since = None;
                    loop {
                        if let Some(m) = s.q.pop_front() {
                            if matches!(m, Msg::Line(_)) {
                                s.lines -= 1;
                                s.writing_since = Some(Instant::now());
                            }
                            break m;
                        }
                        if s.dropped > 0 {
                            // Nothing after the loss to stand before: report
                            // it now.
                            s.writing_since = Some(Instant::now());
                            break Msg::Line(dropped_report(std::mem::take(&mut s.dropped)));
                        }
                        s = q.ready.wait(s).unwrap_or_else(|e| e.into_inner());
                    }
                };
                q.room.notify_all();
                match m {
                    Msg::Line(l) => {
                        if writeln!(out, "{l}").and_then(|_| out.flush()).is_err() {
                            let mut s = q.lock();
                            s.gone = true;
                            s.writing_since = None;
                            drop(s);
                            q.room.notify_all();
                            platform::exit(0);
                            return;
                        }
                    }
                    Msg::Exit(code) => {
                        let _ = out.flush();
                        platform::exit(code);
                        return;
                    }
                    Msg::Flushed(done) => {
                        let _ = out.flush();
                        let _ = done.send(());
                    }
                }
            }
        })
        .expect("rust-toast: can't start the writer thread");
    Out(q)
}

fn serve() {
    WAITS.with(|w| w.set(true)); // this thread reads stdin
    let tx = writer();
    let out = tx.clone();
    let emit: Emit = Arc::new(move |e: Event| out.line(e.to_line()));
    let mut s = Session::new(Global, Config::default(), emit);
    tx.line(s.ready().to_line());
    let mut r = io::stdin().lock();
    loop {
        match protocol::read_line(&mut r, MAX_LINE) {
            Ok(Some(Ok(line))) => {
                if s.line(&line) == Flow::Quit {
                    break;
                }
            }
            Ok(Some(Err(start))) => {
                let e = Event::error(format!("line longer than {MAX_LINE} bytes"), Some(&start));
                tx.line(e.to_line());
            }
            Ok(None) => break, // stdin closed: the app that spawned us is gone
            Err(e) => {
                eprintln!("rust-toast: reading stdin: {e}");
                break;
            }
        }
    }
    // Not behind a stuck write: a full stdout must not hold the exit up.
    tx.exit_soon(0);
}

fn once(o: Once) {
    let tx = writer();
    let out = tx.clone();
    let emit: Emit = Arc::new(move |e: Event| {
        let code = match &e {
            Event::Action { .. } | Event::Click { .. } | Event::Dismiss { .. } => Some(0),
            Event::Error { message, .. } if message.starts_with("not shown") => Some(1),
            Event::Error { .. } => None,
            _ => return, // ready, posted: only the outcome is printed
        };
        out.line(e.to_line());
        if let Some(c) = code {
            out.exit_after(c);
        }
    });
    let mut s = Session::new(Global, Config::default(), emit.clone());
    if !toast::protocol::Backend::supported(s.backend()) {
        emit(Event::Error {
            message: "not shown: no cards on this system".into(),
            id: None,
            command: None,
        });
        return;
    }
    let setup = [
        o.app_name.map(|a| {
            protocol::Command::Configure(Box::new(protocol::ConfigureCmd {
                app_name: Some(a),
                ..Default::default()
            }))
        }),
        o.corner.map(|c| {
            protocol::Command::SetOptions(protocol::OptionsCmd {
                corner: Some(c),
                ..Default::default()
            })
        }),
        Some(protocol::Command::Post(o.post)),
    ];
    for c in setup.into_iter().flatten() {
        if let Err((message, id)) = s.run(c) {
            let message = if message.starts_with("not shown") {
                message
            } else {
                format!("not shown: {message}")
            };
            let e = Event::Error {
                message,
                id,
                command: None,
            };
            emit(e);
            return;
        }
    }
    if o.with_parent {
        // The app that spawned us holds our stdin; when it goes, so does
        // the card (reported as closed, and the emit exits).
        let _ = io::copy(&mut io::stdin().lock(), &mut io::sink());
        for id in s.ids() {
            let _ = s.run(protocol::Command::Close { id });
        }
    }
    // The emit exits once the card is gone; keep the session (and so the
    // process's view of it) alive until then.
    loop {
        std::thread::park();
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use dispatch2::DispatchQueue;
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};

    /// Runs NSApp on the main thread, as Accessory (no Dock icon, no menu
    /// bar, never activated), and f on another thread: the cards' work is
    /// queued on the main dispatch queue, which NSApp serves.
    pub fn run(f: impl FnOnce() + Send + 'static) {
        let mtm = MainThreadMarker::new().expect("rust-toast: main must run on the main thread");
        let app = NSApplication::sharedApplication(mtm);
        app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
        std::thread::Builder::new()
            .name("rust-toast-in".into())
            .spawn(f)
            .expect("rust-toast: can't start the input thread");
        loop {
            app.run(); // returns only if something stops it; run again
        }
    }

    /// Exits on the main thread, between AppKit's events.
    pub fn exit(code: i32) {
        DispatchQueue::main().exec_async(move || std::process::exit(code));
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    /// Runs f here: on Windows the library starts its own card thread.
    pub fn run(f: impl FnOnce() + Send + 'static) {
        f();
        // The writer exits the process once its lines are out.
        loop {
            std::thread::park();
        }
    }

    pub fn exit(code: i32) {
        std::process::exit(code);
    }
}
