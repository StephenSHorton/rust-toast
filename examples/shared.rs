//! One program's cards for trying the shared stack: run it twice, at once,
//! with different names, and both programs' cards go up in one stack in the
//! bottom-right corner (the first to post draws it; the other sends its
//! cards over).
//!
//! `cargo run --example shared -- <name> [light|dark] [cards] [seconds] [delay ms]`
//!
//! For example, in two terminals:
//!
//! ```sh
//! cargo run --example shared -- Hearth dark 2 12
//! cargo run --example shared -- Suzuri light 2 10 600
//! ```

use std::sync::Arc;
use std::time::Duration;

use toast::{Action, CORNER_BOTTOM_RIGHT, Config, Kind, Note, Options, Scheme};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let name = args.get(1).cloned().unwrap_or_else(|| "App".into());
    let light = args.get(2).is_some_and(|s| s == "light");
    let num = |i: usize, d: u64| args.get(i).and_then(|s| s.parse().ok()).unwrap_or(d);
    let (cards, secs, delay) = (num(3, 2), num(4, 10), num(5, 0));
    toast::configure(Config {
        app_name: name.clone(),
        scheme: if light { Scheme::Light } else { Scheme::Dark },
        corner_radius: if light { 10.0 } else { 0.0 },
        // Visible in screenshots, so the stack can be checked by capture.
        capture_exclude: false,
        ..Config::default()
    });
    toast::set_options(Options {
        corner: CORNER_BOTTOM_RIGHT.into(),
        sound: false,
        ..Options::default()
    });
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(delay));
        for i in 1..=cards {
            let n = name.clone();
            toast::post(Note {
                title: format!("{name}: card {i} of {cards}"),
                body: format!("Posted by the {name} program (pid {}).", std::process::id()),
                kind: if i % 2 == 1 {
                    Kind::Success
                } else {
                    Kind::Info
                },
                sticky: true,
                actions: vec![Action::new("Open", move || {
                    println!("{n}: Open pressed on card {i}")
                })],
                on_dismiss: Some(Arc::new(move || println!("dismissed card {i}"))),
                ..Note::default()
            });
            std::thread::sleep(Duration::from_millis(700));
        }
    });
    toast::run_for(Duration::from_secs(secs));
}
