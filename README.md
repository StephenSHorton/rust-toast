# rust-toast

Native notification cards for Rust apps on Windows and macOS. Cards stack
in a corner of the screen you're on, sit over every window, never take
focus, and can carry a thumbnail, up to two buttons, a sound and a burst of
confetti on good news.

![A card on Windows](test-card-windows.png)

They are custom-drawn windows, not the OS notification centre:

- **Windows:** layered, topmost, `WS_EX_NOACTIVATE` tool windows on one
  dedicated UI thread with its own message loop. Real glass (acrylic) on
  Windows 11, solid elsewhere.
- **macOS:** non-activating `NSPanel`s at status-window level, on the
  AppKit main thread your app already runs (Tauri does). Glass through
  `NSVisualEffectView`.
- **Elsewhere:** every call is a no-op.

It's a Rust port of [go-toast](https://github.com/StephenSHorton/go-toast)
v0.2.0. It grew up inside Irori and is shared by suzuri and other apps.

## Use

```toml
[dependencies]
rust-toast = { git = "https://github.com/StephenSHorton/rust-toast", tag = "v0.3.0" }
```

```rust
use toast::{Action, Config, Kind, Note};

// Once, at startup: the look (palette, scheme, corner radius, glass).
toast::configure(Config { app_name: "My App".into(), ..Config::default() });

toast::post(Note {
    title: "Build finished".into(),
    body: "All tests passed.".into(),
    kind: Kind::Success,
    actions: vec![Action::new("Open", || println!("open"))],
    ..Note::default()
});
```

- `Note::id` replaces a live card in place instead of stacking a new one;
  `toast::close(id)` takes it down.
- `sticky` cards stay until clicked or dismissed; others expire (hovering
  pauses the timer).
- `toast::set_options` takes the person's settings: on/off, corner, sound and
  volume, confetti.
- `Config::capture_exclude` (on by default) keeps cards out of screenshots
  and screen recordings.
- `Config::stripe` (on by default) draws the kind-coloured bar down the
  card's left edge; off, the icon alone carries the colour.

Try it: `cargo run --example demo` (`cargo run --example demo 20 6` keeps
it up 20 s with six cards, enough to fold a deck).

## Motion

The stack is a list with a deck in the corner:

- **One column** from the corner out: the oldest card at the corner, the
  newest farthest out.
- **At rest** the newest three are in full. Older ones fold into a deck
  that stays in the corner: the newest of them in front, up to two
  peeking out toward the corner behind it, each a step smaller and
  fainter, cut to the front card's height.
- **Hover** opens the stack. If every card fits the screen in one column,
  they all show; otherwise as many as fit, with a deck at each end,
  opening on the newest. The **wheel** (or a vertical drag on a card)
  moves one card per notch; wheel up goes toward the newer cards in a
  bottom corner, toward the older in a top one.
- **Leaving** folds it again at once, except just after a card was thrown
  away: then it waits 1.2 s, time to come back for the next one. Nothing
  expires while the stack is open.
- **Drag a card sideways** to throw it away (past 110 dip, or a quick
  flick): it follows the pointer and fades, and a throw counts as the
  person's dismiss (`on_dismiss`). Short of that it springs back. The
  first 12 dip of a drag pick its axis for the whole drag; a press dragged
  off a card is never a click.
- Everything moves with a springy 420 ms ease: new cards slide in from the
  screen edge, closing ones slide toward it and fade, thrown ones fly off,
  and the rest glide into their new places. With the OS's reduce-motion
  setting on (Windows: Animation effects off; macOS: Reduce motion) every
  move jumps to its end.

All of it is `Config::motion` (`toast::Motion`), with the designed
values as `Motion::default()`:

| Field | Default | What |
| --- | --- | --- |
| `shown` | 3 | cards in full at rest |
| `max_cards` | 5 | cards up at once; a post past it pushes the oldest non-sticky one off |
| `peek`, `peek_depth` | 12 dip, 2 | how far and how many cards peek out behind a deck's front |
| `peek_shrink`, `peek_fade` | 0.05, 0.22 | each step back: smaller, more transparent |
| `lock` | 12 dip | drag that decides its axis |
| `dismiss_distance`, `flick_speed` | 110 dip, 0.6 dip/ms | a release past either throws the card |
| `drag_fade`, `drag_min_opacity` | 260 dip, 0.2 | a dragged card fades over this distance, to this |
| `scroll_step` | 44 dip | vertical drag per card scrolled |
| `rubber_band` | 0.15 | give of a vertical drag with nothing to scroll |
| `wheel_debounce` | 90 ms | wheel steps closer than this count once |
| `after_throw` | 1200 ms | how long the stack stays open after a throw |
| `duration`, `exit` | 420 ms, 0.8 | every move; leaving takes this fraction |
| `ease`, `ease_out` | `[.2, .9, .25, 1.15]`, `[.2, .8, .2, 1]` | cubic-bezier for moves (springy) and fades |
| `enter_offset`, `enter_scale` | 60 dip, 0.96 | where a new card slides in from |
| `leave_offset`, `throw_offset` | 40 dip, 420 dip | how far closing and thrown cards travel |

`max_cards` keeps the old limit of 5 by default, so a deck holds at most
two cards and the two-ended open stack only shows on a short screen; raise
it to keep more.

**Wheel on Windows:** the cards never take focus. With "Scroll inactive
windows when I hover over them" on (the default since Windows 10) the
wheel arrives at the card under the cursor. With it off, the card thread
installs a low-level mouse hook only while the stack is open, and only
wheel steps over the stack go to it (and not to the focused app).

## Check

```sh
cargo test
cargo clippy --all-targets -- -D warnings
cargo build --example demo
RUST_TOAST_SMOKE=1 cargo test smoke   # Windows: real cards on screen
```

The tests drive the shared engine headless with a recording surface:
layout, decks, hover, the wheel, drags and throws, timings and reduce
motion. The AppKit half (`src/darwin.rs`) runs only on a Mac;
`cargo clippy --target aarch64-apple-darwin --all-targets -- -D warnings`
type-checks it from any OS (`rustup target add aarch64-apple-darwin`),
and [MAC.md](MAC.md) lists what to build and check there.

## Sounds

`sounds/` holds the s&box editor's notification sounds, MIT, from
Facepunch Studios ([sounds/NOTICE.md](sounds/NOTICE.md)).

## Licence

MIT.
