//! `rust-toast`: the cards for programs not written in Rust (PROTOCOL.md).
//!
//! `rust-toast serve` reads newline-delimited JSON commands on stdin and
//! writes newline-delimited JSON events on stdout (logs on stderr). It
//! exits when stdin closes, so it never outlives the program that spawned
//! it, and on `{"cmd":"quit"}`.
//!
//! `rust-toast --title T [--body B ...]` shows one card, waits until it is
//! gone and prints how it went, as one event line.
//!
//! On macOS the main thread runs NSApp (Accessory: no Dock icon, never
//! active) and the commands are read on another thread; on Windows the
//! library runs its own card thread. Elsewhere it reports
//! `"supported": false` and takes commands as no-ops.

use std::io::{self, Write};
use std::sync::Arc;
use std::sync::mpsc;

use toast::Config;
use toast::protocol::{
    self, ActionCmd, Emit, Event, Flow, Global, KindName, MAX_LINE, PostCmd, Session,
};

const USAGE: &str = "\
usage: rust-toast serve
       rust-toast --title TEXT [--body TEXT] [--kind info|success|warn|error]
                  [--sticky] [--timeout SECONDS] [--silent]
                  [--action ID:LABEL]... [--thumbnail PATH]
                  [--corner top-right|bottom-right|top-left|bottom-left]
                  [--app-name NAME]
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

  --version          Prints the version.
  --help             Prints this.

Exit status: 0 when the card went (or serve ended), 1 when it could not be
shown, 2 for bad arguments.";

#[derive(Debug, Default)]
struct Once {
    post: PostCmd,
    corner: Option<String>,
    app_name: Option<String>,
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
            "--corner" => o.corner = Some(val(a)?),
            "--app-name" => o.app_name = Some(val(a)?),
            _ => return Err(format!("unknown argument {a:?}")),
        }
    }
    if o.post.title.trim().is_empty() && o.post.body.trim().is_empty() {
        return Err("--title or --body is needed".into());
    }
    Ok(Some(Mode::Once(Box::new(o))))
}

fn main() {
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
}

/// Events go through one writer thread: a line at a time, flushed, so the
/// card threads never wait on a slow reader. A write that fails means the
/// reader is gone: the program exits.
fn writer() -> mpsc::Sender<Msg> {
    let (tx, rx) = mpsc::channel::<Msg>();
    std::thread::Builder::new()
        .name("rust-toast-out".into())
        .spawn(move || {
            let mut out = io::stdout().lock();
            for m in rx {
                match m {
                    Msg::Line(l) => {
                        if writeln!(out, "{l}").and_then(|_| out.flush()).is_err() {
                            platform::exit(0);
                            return;
                        }
                    }
                    Msg::Exit(code) => {
                        let _ = out.flush();
                        platform::exit(code);
                        return;
                    }
                }
            }
        })
        .expect("rust-toast: can't start the writer thread");
    tx
}

fn serve() {
    let tx = writer();
    let out = tx.clone();
    let emit: Emit = Arc::new(move |e: Event| {
        let _ = out.send(Msg::Line(e.to_line()));
    });
    let mut s = Session::new(Global, Config::default(), emit);
    let _ = tx.send(Msg::Line(s.ready().to_line()));
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
                let _ = tx.send(Msg::Line(e.to_line()));
            }
            Ok(None) => break, // stdin closed: the app that spawned us is gone
            Err(e) => {
                eprintln!("rust-toast: reading stdin: {e}");
                break;
            }
        }
    }
    let _ = tx.send(Msg::Exit(0));
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
        let _ = out.send(Msg::Line(e.to_line()));
        if let Some(c) = code {
            let _ = out.send(Msg::Exit(c));
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
