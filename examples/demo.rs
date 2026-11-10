//! Posts a few cards so you can see and click them:
//! `cargo run --example demo [seconds] [cards]` (default 8 s, 2 cards).
//! Past three cards the older ones fold into the corner deck; hover to open
//! the stack, drag a card sideways to throw it away.
//!
//! On the Mac the example runs the AppKit loop itself (`toast::run_for`);
//! inside a Tauri app the cards use the loop Tauri already runs.

use std::time::Duration;

use toast::{Action, Config, Kind, Motion, Note};

fn main() {
    let arg = |i: usize| std::env::args().nth(i).and_then(|s| s.parse::<u64>().ok());
    let secs = arg(1).unwrap_or(8);
    let cards = arg(2).unwrap_or(2) as usize;
    toast::configure(Config {
        app_name: "rust-toast demo".into(),
        // Visible in screenshots, so the cards can be checked by capture.
        capture_exclude: false,
        motion: Motion {
            max_cards: cards.max(5),
            ..Motion::default()
        },
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
        actions: vec![
            Action::new("Open", || println!("open")),
            Action {
                label: "Later".into(),
                run: None,
            },
        ],
        on_click: Some(std::sync::Arc::new(|| println!("card clicked"))),
        ..Note::default()
    });
    // The rest arrive one by one, so each slides in and the deck folds.
    std::thread::spawn(move || {
        for i in 3..=cards {
            std::thread::sleep(Duration::from_millis(500));
            toast::post(Note {
                title: format!("Card {i}"),
                body: "Drag me sideways to throw me away.".into(),
                sticky: true,
                on_dismiss: Some(std::sync::Arc::new(move || println!("threw card {i}"))),
                ..Note::default()
            });
        }
    });
    toast::run_for(Duration::from_secs(secs));
}
