# toast on macOS: what the Mac must compile and check

`src/darwin.rs` (the AppKit cards) was written on the PC against the objc2
sources in the cargo registry; it builds and passes clippy on the Mac, and
since 0.3.0 the PC type-checks it too (`rustup target add
aarch64-apple-darwin`, then `cargo clippy --target aarch64-apple-darwin
--all-targets -- -D warnings`). Everything else
in the crate (model, layout, sounds, the shared `Engine` the Win32 side
also drives) is platform-neutral and tested on the PC.

## Checked on a Mac (macOS 26.6.2, Apple silicon, 2026-11-10)

Done without touching the mouse or keyboard: commands, passive
`screencapture` shots and a passive window check
(`CGWindowListCopyWindowInfo`, `NSWorkspace.frontmostApplication`,
`NSRunningApplication.isActive`, `NSScreen.visibleFrame`).

- **Section 1:** `cargo build`, `cargo clippy --all-targets -- -D warnings`
  and `cargo test` are clean on 0.3.0 as written; `darwin.rs` needed no
  fixes. Also clean with `--features cli`, and `cargo fmt --check`.
- **Demo, 2 cards** (`cargo run --example demo 10 2`): both cards drawn as
  designed (dark, success stripe green with a check, info stripe blue with
  `info.circle`, two-line body, Open blue and Later). Top-right, 20 pt in
  from the right and from the bottom of the menu bar (`visibleFrame` top),
  inside the visible frame; 340 pt wide (62 pt tall with a one-line body,
  115 pt with two lines and buttons), 8 pt apart, the oldest at the corner.
  Window layer 25 (`NSStatusWindowLevel`), alpha 245/255.
- **Focus:** the frontmost app stayed the terminal throughout; the demo
  process ran with the Accessory policy and was never active
  (`isActive` false).
- **Demo, 7 cards** (`demo 9 7`): past three the older cards fold into the
  corner deck: the front card in full, two slivers toward the corner
  behind it (323 x 59 at alpha 0.75, 306 x 56 at alpha 0.54), the newest
  three in full below.
- **Capture:** with `capture_exclude` on (the default) the panel's sharing
  state is None and `screencapture` shows nothing where the card is; off,
  it is ReadOnly and the card is in the shot. A `configure` that turns
  it off later applies to new cards only: a card already up stays out of
  captures.
- **The `rust-toast` program** (`--features cli`): real cards, bottom-right
  corner, PNG and JPEG thumbnails, expiry, `close`, Accessory policy and
  never active; it exits when its stdin closes, also when the app that
  spawned it is killed. `RUST_TOAST_SMOKE=1 cargo test --features cli
  cli_smoke` passes (real cards, posted, expired, closed).
- **Seen in passing:** a card that lands where the pointer rests counts
  as hovering the stack, so nothing expires until the pointer moves off
  (the same as hovering; only a card still sliding in is ignored).

Still needs a person's eyes and hand (nothing here drives the mouse):
hover (brightening x, the stack opening and folding, leaving across a gap),
clicks on Open, Later, the body and the x, Control-click and right click,
a press dragged off a card, sideways drags and throws (`threw card N`),
the hold after a throw, the wheel and vertical drag on a tall stack
(natural scrolling on and off), another Space and a full-screen app, a
screen unplugged or a resolution change, Reduce motion, and the
appearance flip with the `System` scheme (checked earlier in Irori).
`keyWindow`/`isKeyWindow` can only be read inside the process; from
outside, the app never became active and the frontmost app never changed.

## 1. Compile

```sh
cargo build -p rust-toast
cargo clippy -p rust-toast --all-targets -- -D warnings
cargo test -p rust-toast            # model.rs and engine.rs run on any OS
```

Expect API-name fixes in `darwin.rs`; the shape is a line-for-line port of
go-toast v0.2.0 `toast_darwin.m` / `toast_darwin.go`. The calls most likely
to need adjusting, each checked only by reading generated sources:

- `define_class!` for `ToastPanel` (super `NSPanel, NSWindow, NSResponder,
  NSObject`) and `ToastView` (super `NSView, NSResponder, NSObject`, ivars
  `ViewIvars`), both `MainThreadOnly`; `msg_send![super(self), updateTrackingAreas]`
  and `msg_send![super(this), initWithFrame: frame]`.
- `msg_send![ToastPanel::alloc(mtm), initWithContentRect:styleMask:backing:defer:]`.
- `NSDictionary::from_slices` for the string attributes, `Retained`-to-`&AnyObject`
  coercions (`f.as_ref()`), and the `NSFontAttributeName` family statics.
- `NSStringNSExtendedStringDrawing::{drawWithRect_options_attributes_context,
  boundingRectWithSize_options_attributes_context}`, `NSStringDrawing::sizeWithAttributes`.
- `NSImage::imageWithSystemSymbolName_accessibilityDescription`,
  `NSImageSymbolConfiguration::configurationWithPointSize_weight`,
  `imageWithSymbolConfiguration`; the icon tint uses `lockFocus` (go-toast
  used `imageWithSize:flipped:drawingHandler:`; either is fine).
- `RcBlock` closures for `NSTimer::timerWithTimeInterval_repeats_block` and
  `NSNotificationCenter::addObserverForName_object_queue_usingBlock`.
- `NSSound::initWithData` + `play` (go-toast used AVAudioPlayer on a serial
  queue; `objc2-av-foundation` is not in Cargo.lock, so NSSound on the main
  queue replaces it; switch if a sound clips or lags).
- `DispatchQueue::main().exec_async` and `after` (dispatch2 0.3.1).
- `darwin::run_for` (behind `toast::run_for`, used by the `demo`
  example): `NSApplication` with the Accessory policy, `run`, and a timed
  `stop` plus posted `ApplicationDefined` event (go-toast's `Main`).

## 2. Check by hand

`cargo run --example demo 8` shows two cards (a success card and a
sticky one with Open / Later). With it up, check what go-toast's
`toast_smoke_darwin_test.go` asserted:

- the card is top-right on the screen with the mouse, inside `visibleFrame`
  (below the menu bar), 340 x 94 pt, dark, blue stripe, `info.circle` icon,
  two-line ellipsized body, Open (blue) and Later buttons;
- the frontmost app did not change and the app did not become active or key
  (`NSWorkspace.frontmostApplication`, `NSApp.isActive`, `keyWindow` nil);
- the panel's level is `NSStatusWindowLevel` (25), it is not key or main, and
  it shows on another Space and over a full-screen app;
- hover brightens the x and pauses expiry; a press dragged off the card is no
  click; Control-click and right click dismiss; a body click with no action
  dismisses; Open prints `open`;
- with `capture_exclude` true (the `Config` default; the demo turns it
  off), `screencapture` does not show the card
  (`NSWindowSharingNone`);
- a screen unplugged or a resolution change under a sticky card moves the
  stack (`NSApplicationDidChangeScreenParametersNotification`).

## 2b. The look

Each panel's content view is a plain layer-backed view holding an
`NSVisualEffectView` (BehindWindow, state Active since the panel is never
key, material Popover) and, over it, the drawn `ToastView` (which takes the
mouse). The blur is rounded with a cap-inset stretchable mask image, the
`ToastView` layer with its corner radius. Glass paints the palette's tint at
the look's opacity over the blur; solid (no glass, or "Reduce transparency")
hides the blur and paints the opaque background. The panel's appearance is
Aqua or Dark Aqua per the card. A restyle (theme, mode, radius, glass) on a
card already up redoes all of it and invalidates the shadow.

With the "system" scheme, `os_dark` reads the `AppleInterfaceStyle`
default (not NSApp's appearance, which Tauri may pin to the app's mode);
`AppleInterfaceThemeChangedNotification` (distributed, observed with
DeliverImmediately: AppKit otherwise holds it while the app is inactive,
which is when cards are up) queues syncs so cards flip at once.

Check it: with a card up, flip System Settings > Appearance while the
config's scheme is `System`: the card follows within a second (checked on
macOS 26.6 in Irori).

## 2c. Motion (0.3.0: list + deck)

The motion model lives in the shared engine and is tested headless on the
PC; `darwin.rs` only gives it windows. As of 0.3.0 the crate type-checks
for macOS from the PC (`cargo clippy --target aarch64-apple-darwin
--all-targets -- -D warnings` is clean), so the objc2 names and signatures
below compile; what nobody has seen yet is how they behave. The new or
changed AppKit calls, each only compiled, never run:

- `MacSurface::place` now runs every frame (60 Hz) while a card moves:
  `setFrame_display(frame, resized)` (display only when the size changed)
  plus `invalidateShadow` on a resize. A deck's peeking cards are smaller
  panels (scaled 0.95, 0.90), not a layer transform.
- `MacSurface::restack`: `orderFrontRegardless` on each shown panel,
  bottom to top, whenever the stacking order changes (the deck under the
  shown cards, a dragged card on top, a thrown one over everything).
- `MacSurface::frames`: a second repeating `NSTimer` (16 ms, common modes)
  calling `Engine::frame`; `timer` and `frames` share `repeating()`.
- `MacSurface::cursor`: `NSEvent::mouseLocation()` flipped to top-down
  against the primary screen's height. The frames poll it while the stack
  is open to see the cursor leave the stack (cards are separate panels
  with gaps, so a per-panel `mouseExited:` alone would fold it between
  cards).
- `MacSurface::reduce_motion`:
  `NSWorkspace::accessibilityDisplayShouldReduceMotion`, read every sync.
- `ToastView` `mouseDragged:` sends moves to the engine (AppKit sends a
  press's drags to the view it started on, even off it: the Mac's
  capture). The panel moves under the pointer while dragged; check the
  view's `locationInWindow` stays consistent (the engine adds the
  panel's current origin).
- `ToastView` `scrollWheel:` reads `scrollingDeltaY()`: positive is
  "up" (`Wheel::Up`). Check the direction with natural scrolling both
  on and off: in a bottom corner, scrolling up should bring newer cards
  (the ones farther from the corner) into view. Flip the sign in
  `scroll_wheel` if it's backwards. A trackpad sends a stream of
  deltas; the engine's 90 ms debounce makes it one card per gesture
  step, which may feel slow or fast on a trackpad.
- `draw_card` draws only the body (fill and border) for a `folded`
  look. A card mid-way between sizes draws its content at full size
  clipped by the smaller panel (the Win32 side draws it scaled); that
  lasts only while a card folds or unfolds.

Check by hand with `cargo run --example demo 30 7` (seven cards, 30 s):

- the first cards slide in from the screen edge (right side for a right
  corner), the newest lands farthest from the corner;
- past three, the older cards fold into a deck in the corner: the
  newest folded one in front in full, two slivers peeking toward the
  corner behind it, smaller and fainter;
- hovering anywhere on the stack opens every card into one column,
  oldest at the corner; moving the cursor between two cards (over the
  gap) keeps it open; leaving folds it at once;
- dragging a card sideways makes it follow and fade; released past
  about 110 pt or flicked it flies off and the demo prints
  `threw card N`; short of that it springs back; a press that moved is
  never a click;
- after a throw, moving off the stack keeps it open about 1.2 s;
  coming back within that keeps it open;
- with System Settings > Accessibility > Display > Reduce motion on,
  cards jump into place with no sliding;
- nothing steals focus during any of it (the checks in section 2);
- on a short screen (or `Motion { max_cards: 20, .. }` and many cards)
  the open stack shows a deck at each end and the wheel or a vertical
  drag moves through it one card at a time.

Watch for re-entrancy: the engine calls the surface while it holds its
window-state lock. If AppKit ever delivers a mouse event synchronously
inside `setFrame_display` or `orderFrontRegardless` (it shouldn't; events
are queued), the main thread would deadlock on that lock.

## 3. Port the darwin smoke test

go-toast's `toast_smoke_darwin_test.go` needs an NSApp loop on the main
thread, which `cargo test` doesn't give. Port it as a `harness = false`
test (or an example) that runs `NSApplication` on the main thread, posts
the three smoke cards from another thread, and asserts the list above
through `toast::card_keys()` / `toast::visible()` plus a small test-only
panel-info hook in `darwin.rs` (go-toast's `toast_test_info`).

## 4. In the app

Nothing to call: Tauri runs NSApp on the main thread, and `toast::post`
queues its work on the main dispatch queue. Never start a second NSApp
loop.
