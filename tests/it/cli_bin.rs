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
    // One card at a time: the first lands in the corner. A pointer resting
    // where a card lands counts as hovering the stack, which pauses expiry
    // (by design), so keep the pointer off the top-right corner.
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
