//! Posts a few cards so you can see and click them:
//! `cargo run --example demo [seconds]` (default 8).
//!
//! On the Mac the example runs the AppKit loop itself (`toast::run_for`);
//! inside a Tauri app the cards use the loop Tauri already runs.

use std::time::Duration;

use toast::{Action, Config, Kind, Note};

fn main() {
    let secs: u64 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(8);
    toast::configure(Config {
        app_name: "rust-toast demo".into(),
        // Visible in screenshots, so the cards can be checked by capture.
        capture_exclude: false,
        ..Config::default()
    });
    toast::post(Note {
        title: "Build finished".into(),
        body: "All 152 tests passed in 4.2 s.".into(),
        kind: Kind::Success,
        ..Note::default()
    });
    toast::post(Note {
        id: "ask".into(),
        title: "Claude is waiting on you".into(),
        body: "Pane 2 asked a question. Open it to answer, or dismiss this card.".into(),
        kind: Kind::Info,
        sticky: true,
        actions: vec![Action::new("Open", || println!("open")), Action { label: "Later".into(), run: None }],
        on_click: Some(std::sync::Arc::new(|| println!("card clicked"))),
        ..Note::default()
    });
    toast::run_for(Duration::from_secs(secs));
}
