//! The `rust-toast` program itself, spawned as an app would spawn it. The
//! default tests post no card, so they run on a headless CI machine: the
//! ready event, an error for a bad line, `list`, and the exit when stdin
//! closes or on quit. With RUST_TOAST_SMOKE=1 (Windows or macOS) real cards
//! go up: posted, expired, closed, and focus never moves to the program.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_rust-toast");

struct Program {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: mpsc::Receiver<String>,
}

impl Program {
    fn serve() -> Program {
        let mut child = Command::new(BIN)
            .arg("serve")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("spawn rust-toast");
        let stdin = child.stdin.take();
        let out = BufReader::new(child.stdout.take().unwrap());
        let (tx, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for l in out.lines() {
                let Ok(l) = l else { break };
                if tx.send(l).is_err() {
                    break;
                }
            }
        });
        Program {
            child,
            stdin,
            lines,
        }
    }

    fn send(&mut self, line: &str) {
        let s = self.stdin.as_mut().expect("stdin open");
        writeln!(s, "{line}").unwrap();
        s.flush().unwrap();
    }

    fn event(&self) -> serde_json::Value {
        let l = self
            .lines
            .recv_timeout(Duration::from_secs(10))
            .expect("an event line within 10 s");
        serde_json::from_str(&l).unwrap_or_else(|e| panic!("not JSON: {l:?}: {e}"))
    }

    /// Waits for the program to end by itself.
    fn exits(&mut self, within: Duration) -> std::process::ExitStatus {
        let end = Instant::now() + within;
        loop {
            if let Some(s) = self.child.try_wait().unwrap() {
                return s;
            }
            if Instant::now() > end {
                let _ = self.child.kill();
                panic!("rust-toast still running after {within:?}");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Program {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn version_and_help() {
    let o = Command::new(BIN).arg("--version").output().unwrap();
    assert!(o.status.success());
    assert_eq!(
        String::from_utf8_lossy(&o.stdout).trim(),
        format!("rust-toast {}", env!("CARGO_PKG_VERSION"))
    );
    let o = Command::new(BIN).arg("--help").output().unwrap();
    assert!(o.status.success());
    assert!(String::from_utf8_lossy(&o.stdout).contains("serve"));
    let o = Command::new(BIN).output().unwrap();
    assert_eq!(o.status.code(), Some(2), "no arguments: usage");
    let o = Command::new(BIN)
        .args(["--title", "x", "--bogus"])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(2));
}

#[test]
fn serve_reports_ready_errors_and_exits_when_stdin_closes() {
    let mut p = Program::serve();
    let r = p.event();
    assert_eq!(r["event"], "ready");
    assert_eq!(r["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(r["protocol"], 1);
    let supported = cfg!(any(windows, target_os = "macos"));
    assert_eq!(r["supported"], supported);

    p.send("this is not json");
    let e = p.event();
    assert_eq!(e["event"], "error");
    assert_eq!(e["command"], "this is not json");

    p.send(r#"{"cmd":"close","id":"nothing"}"#);
    p.send(r#"{"cmd":"list","extra":true}"#);
    let e = p.event();
    assert_eq!(e["event"], "cards");
    assert_eq!(e["ids"], serde_json::json!([]));

    // A line far past the limit is one error, and the next line still works.
    let long = "x".repeat(toast::protocol::MAX_LINE + 10);
    p.send(&long);
    let e = p.event();
    assert_eq!(e["event"], "error");
    assert!(e["command"].as_str().unwrap().len() <= 4 * toast::protocol::MAX_ECHO + 4);
    p.send(r#"{"cmd":"list"}"#);
    assert_eq!(p.event()["event"], "cards");

    drop(p.stdin.take()); // the app that spawned it is gone
    assert!(p.exits(Duration::from_secs(10)).success());
}

#[test]
fn serve_exits_on_quit() {
    let mut p = Program::serve();
    assert_eq!(p.event()["event"], "ready");
    p.send(r#"{"cmd":"quit"}"#);
    assert!(p.exits(Duration::from_secs(10)).success());
}

#[cfg(any(windows, target_os = "macos"))]
#[test]
fn cli_smoke() {
    if std::env::var("RUST_TOAST_SMOKE").as_deref() != Ok("1") {
        eprintln!("skipped: set RUST_TOAST_SMOKE=1");
        return;
    }
    let mut p = Program::serve();
    assert_eq!(p.event()["event"], "ready");
    // Visible in screenshots, silent.
    p.send(r#"{"cmd":"configure","app_name":"rust-toast smoke","capture_exclude":false}"#);
    p.send(r#"{"cmd":"set_options","sound":false}"#);
    // One card at a time, in the corner: a pointer resting where a card
    // lands was seen to pause expiry like a hover (MAC.md), so keep the
    // pointer off the top-right corner.
    p.send(r#"{"cmd":"post","id":"quick","title":"Expires","body":"in half a second","timeout_ms":500}"#);
    assert_eq!(
        p.event(),
        serde_json::json!({"event":"posted","id":"quick"})
    );
    assert_eq!(
        p.event(),
        serde_json::json!({"event":"dismiss","id":"quick","how":"timeout"})
    );
    p.send(
        r#"{"cmd":"post","id":"ask","title":"Sticky","body":"closed by the test","kind":"warn","sticky":true, "actions":[{"id":"open","label":"Open"},{"id":"later","label":"Later"}]}"#,
    );
    assert_eq!(p.event(), serde_json::json!({"event":"posted","id":"ask"}));
    p.send(r#"{"cmd":"list"}"#);
    assert_eq!(
        p.event(),
        serde_json::json!({"event":"cards","ids":["ask"]})
    );
    p.send(r#"{"cmd":"close","id":"ask"}"#);
    assert_eq!(
        p.event(),
        serde_json::json!({"event":"dismiss","id":"ask","how":"closed"})
    );
    drop(p.stdin.take());
    assert!(p.exits(Duration::from_secs(10)).success());
}

/// `serve` with its stdout piped and never read, as a fire-and-forget app
/// would leave it.
fn serve_unread() -> (Child, ChildStdin, std::process::ChildStdout) {
    let mut child = Command::new(BIN)
        .arg("serve")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn rust-toast");
    let stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    (child, stdin, stdout)
}

/// Waits for child to end by itself; kills it and fails if it doesn't.
fn ends_within(child: &mut Child, within: Duration) -> std::process::ExitStatus {
    let end = Instant::now() + within;
    loop {
        if let Some(s) = child.try_wait().unwrap() {
            return s;
        }
        if Instant::now() > end {
            let _ = child.kill();
            let _ = child.wait();
            panic!("rust-toast still running after {within:?}");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Far more answers than any pipe holds, so the program's stdout is full
/// and stays full: the app never reads it.
fn flood(stdin: &mut ChildStdin) {
    let lines = "{\"cmd\":\"list\"}\n".repeat(20_000);
    stdin.write_all(lines.as_bytes()).unwrap();
    stdin.flush().unwrap();
}

#[test]
fn serve_quits_although_stdout_is_never_read() {
    let (mut child, mut stdin, stdout) = serve_unread();
    flood(&mut stdin);
    writeln!(stdin, r#"{{"cmd":"quit"}}"#).unwrap();
    stdin.flush().unwrap();
    // stdin and stdout stay open: the app is alive and just not reading.
    let s = ends_within(&mut child, Duration::from_secs(5));
    assert!(s.success(), "{s:?}");
    drop((stdin, stdout));
}

#[test]
fn serve_ends_with_stdin_although_stdout_is_never_read() {
    let (mut child, mut stdin, stdout) = serve_unread();
    flood(&mut stdin);
    drop(stdin);
    let s = ends_within(&mut child, Duration::from_secs(5));
    assert!(s.success(), "{s:?}");
    drop(stdout);
}

#[test]
fn a_bad_corner_is_a_bad_argument() {
    let o = Command::new(BIN)
        .args(["--title", "x", "--corner", "middle", "--silent"])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(2), "exit 2 for bad arguments");
    assert!(o.stdout.is_empty(), "no card, no event");
    assert!(String::from_utf8_lossy(&o.stderr).contains("--corner"));
}

/// Where there are no cards, one card can't be shown: exit 1 and one error.
#[cfg(not(any(windows, target_os = "macos")))]
#[test]
fn one_card_exits_1_where_there_are_no_cards() {
    let o = Command::new(BIN)
        .args(["--title", "x", "--silent"])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(1));
    let out = String::from_utf8_lossy(&o.stdout);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 1, "{out:?}");
    let e: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(e["event"], "error");
    assert!(e["message"].as_str().unwrap().starts_with("not shown"));
}

/// `--with-parent`: a one-shot card ends when its stdin closes (the app
/// that spawned it is gone), reported as closed. A real card goes up.
#[cfg(any(windows, target_os = "macos"))]
#[test]
fn one_card_with_parent_ends_when_stdin_closes() {
    if std::env::var("RUST_TOAST_SMOKE").as_deref() != Ok("1") {
        eprintln!("skipped: set RUST_TOAST_SMOKE=1");
        return;
    }
    let mut child = Command::new(BIN)
        .args([
            "--title",
            "With parent",
            "--body",
            "closed when its stdin closes",
            "--sticky",
            "--silent",
            "--corner",
            "bottom-left",
            "--with-parent",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn rust-toast");
    std::thread::sleep(Duration::from_millis(800));
    assert!(child.try_wait().unwrap().is_none(), "the sticky card is up");
    drop(child.stdin.take()); // the parent is gone
    let s = ends_within(&mut child, Duration::from_secs(5));
    let mut out = String::new();
    std::io::Read::read_to_string(&mut child.stdout.take().unwrap(), &mut out).unwrap();
    assert!(s.success(), "{s:?} {out:?}");
    assert_eq!(
        out.trim(),
        r#"{"event":"dismiss","id":"auto-1","how":"closed"}"#
    );
}

/// The error line whose echo marks the end of a burst.
const SENTINEL: &str = "end of the burst";

/// A reading app that sends a burst far past the queue (4096) in one write
/// gets every answer, in order: the input thread waits for the writer
/// instead of dropping. The app reads steadily but a little slower than
/// the program answers, as a real one would.
#[test]
fn serve_answers_every_command_of_a_burst_in_order() {
    let (mut child, mut stdin, stdout) = serve_unread();
    let (tx, lines) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        for (i, l) in BufReader::new(stdout).lines().enumerate() {
            let Ok(l) = l else { break };
            if tx.send(l).is_err() {
                break;
            }
            if i % 200 == 0 {
                std::thread::sleep(Duration::from_millis(5)); // about 0.5 s in all
            }
        }
    });
    let mut burst = "{\"cmd\":\"list\"}\n".repeat(20_000);
    burst.push_str(SENTINEL);
    burst.push('\n');
    stdin.write_all(burst.as_bytes()).unwrap();
    stdin.flush().unwrap();
    let event = || -> serde_json::Value {
        let l = lines
            .recv_timeout(Duration::from_secs(10))
            .expect("an event line within 10 s");
        serde_json::from_str(&l).unwrap_or_else(|e| panic!("not JSON: {l:?}: {e}"))
    };
    assert_eq!(event()["event"], "ready");
    for i in 0..20_000 {
        let e = event();
        assert_eq!(e["event"], "cards", "answer {i} of 20,000: {e}");
    }
    let e = event();
    assert_eq!(e["event"], "error");
    assert_eq!(e["command"], SENTINEL, "after the 20,000 answers: {e}");
    drop(stdin);
    let s = ends_within(&mut child, Duration::from_secs(10));
    assert!(s.success(), "{s:?}");
}

/// The count in a "N events dropped" error, if e is one.
fn dropped(e: &serde_json::Value) -> Option<u64> {
    let m = e["message"].as_str()?;
    let (n, rest) = m.split_once(' ')?;
    (e["event"] == "error" && rest.starts_with("event") && rest.contains("dropped"))
        .then(|| n.parse().ok())
        .flatten()
}

/// An app that stops reading stdout can't make the program hold more than
/// its queue: once a write has been stuck for a moment, answers are dropped
/// and counted, and when the app reads again an error says how many. Every
/// answer is either read or counted.
#[test]
fn serve_with_a_stalled_reader_drops_counts_and_reports() {
    let (mut child, mut stdin, stdout) = serve_unread();
    flood(&mut stdin);
    // Well past the stall window, with stdout still unread.
    std::thread::sleep(Duration::from_millis(1500));
    let (tx, lines) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        for l in BufReader::new(stdout).lines() {
            let Ok(l) = l else { break };
            if tx.send(l).is_err() {
                break;
            }
        }
    });
    // The app reads again: one more answer, then the sentinel.
    writeln!(stdin, "{{\"cmd\":\"list\"}}\n{SENTINEL}").unwrap();
    stdin.flush().unwrap();
    let (mut answers, mut lost, mut ready) = (0u64, 0u64, 0);
    loop {
        let l = lines
            .recv_timeout(Duration::from_secs(10))
            .expect("a line within 10 s");
        let e: serde_json::Value = serde_json::from_str(&l).unwrap();
        if e["event"] == "ready" {
            ready += 1;
        } else if e["event"] == "cards" {
            answers += 1;
        } else if let Some(n) = dropped(&e) {
            lost += n;
        } else {
            assert_eq!(e["command"], SENTINEL, "{e}");
            break;
        }
    }
    assert_eq!(ready, 1);
    assert_eq!(
        answers + lost,
        20_001,
        "{answers} read, {lost} reported dropped"
    );
    assert!(lost > 0, "nothing dropped although stdout went unread");
    // The queue (4096) plus what the pipe holds, not all 20,000.
    assert!(
        answers < 12_000,
        "{answers} answers held for an unread stdout"
    );
    drop(stdin);
    let s = ends_within(&mut child, Duration::from_secs(5));
    assert!(s.success(), "{s:?}");
}

/// Quit right after a burst into an unread stdout ends the program in
/// about half a second (PROTOCOL.md), not the 5 s the tests above allow.
#[test]
fn serve_quits_promptly_after_a_burst_into_an_unread_stdout() {
    let (mut child, mut stdin, stdout) = serve_unread();
    flood(&mut stdin);
    let sent = Instant::now();
    writeln!(stdin, r#"{{"cmd":"quit"}}"#).unwrap();
    stdin.flush().unwrap();
    let s = ends_within(&mut child, Duration::from_millis(1500));
    assert!(s.success(), "{s:?}");
    eprintln!("quit took {:?}", sent.elapsed());
    drop((stdin, stdout));
}
