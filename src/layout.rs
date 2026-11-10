//! Card geometry in dip (go-toast layout.go). A card is the card color, a
//! 1px border, a kind-colored stripe and icon on the left.

use crate::note::{CORNER_BOTTOM_LEFT, CORNER_TOP_LEFT, CORNER_TOP_RIGHT};

/// A cubic-bezier easing curve at progress x (0..1), as CSS's
/// `cubic-bezier(x1, y1, x2, y2)`: the curve's y where its x is x. y may
/// overshoot past 1 (a spring's landing).
pub fn ease(c: [f64; 4], x: f64) -> f64 {
    let x = x.clamp(0.0, 1.0);
    if x <= 0.0 || x >= 1.0 {
        return x;
    }
    let [x1, y1, x2, y2] = c;
    let bez = |a: f64, b: f64, t: f64| {
        let u = 1.0 - t;
        3.0 * u * u * t * a + 3.0 * u * t * t * b + t * t * t
    };
    // x(t) rises from 0 to 1 (x1, x2 in 0..1): bisect for t.
    let (mut lo, mut hi) = (0.0, 1.0);
    let mut t = x;
    for _ in 0..40 {
        let v = bez(x1, x2, t);
        if (v - x).abs() < 1e-7 {
            break;
        }
        if v < x {
            lo = t;
        } else {
            hi = t;
        }
        t = (lo + hi) / 2.0;
    }
    bez(y1, y2, t)
}

pub const CARD_WIDTH_DIP: f64 = 340.0;
pub const CARD_GAP_DIP: f64 = 8.0;
/// From the work area's edges.
pub const CARD_MARGIN_DIP: f64 = 20.0;

pub const STRIPE_DIP: f64 = 3.0;
pub const ICON_LEFT_DIP: f64 = 15.0;
pub const ICON_SIZE_DIP: f64 = 15.0;
pub const TEXT_LEFT_DIP: f64 = 40.0;
pub const TEXT_RIGHT_DIP: f64 = 14.0;
pub const PAD_TOP_DIP: f64 = 11.0;
pub const PAD_BOTTOM_DIP: f64 = 12.0;
/// The title row (Segoe UI Semibold 14).
pub const TITLE_DIP: f64 = 21.0;
pub const TITLE_SIZE_DIP: f64 = 14.0;
/// Between title and body.
pub const BODY_GAP_DIP: f64 = 1.0;
/// A body line (Segoe UI 12.5).
pub const BODY_LINE_DIP: f64 = 17.0;
pub const BODY_SIZE_DIP: f64 = 12.5;
pub const MAX_BODY_LINES: usize = 2;

/// The thumbnail under the body: at most the text column wide and this
/// tall, never scaled up past half its pixel size (a Retina-sized shot).
pub const THUMB_MAX_H_DIP: f64 = 120.0;
/// Above the thumbnail.
pub const THUMB_GAP_DIP: f64 = 8.0;

/// The text column's width.
pub const TEXT_WIDTH_DIP: f64 = CARD_WIDTH_DIP - TEXT_LEFT_DIP - TEXT_RIGHT_DIP;

/// The thumbnail's size in dip for an image w x h px: it keeps its aspect,
/// fits the slot, and is never drawn bigger than 1 dip per 2 px (so a
/// small image stays small instead of going blurry). (0, 0) for an empty
/// image.
pub fn thumb_size_dip(w: u32, h: u32) -> (f64, f64) {
    if w == 0 || h == 0 {
        return (0.0, 0.0);
    }
    let (w, h) = (w as f64, h as f64);
    let s = (TEXT_WIDTH_DIP / w).min(THUMB_MAX_H_DIP / h).min(0.5);
    ((w * s).round().max(1.0), (h * s).round().max(1.0))
}

/// The x's hit box, in the top-right corner.
pub const CLOSE_SIZE_DIP: f64 = 24.0;
pub const CLOSE_INSET_DIP: f64 = 5.0;
/// The title keeps clear of the x.
pub const TITLE_RIGHT_DIP: f64 = CLOSE_SIZE_DIP + CLOSE_INSET_DIP + 4.0;

/// Above the button row.
pub const BUTTON_GAP_DIP: f64 = 10.0;
pub const BUTTON_DIP: f64 = 26.0;
/// Text inset on each side.
pub const BUTTON_PAD_DIP: f64 = 12.0;
pub const BUTTON_MIN_DIP: f64 = 64.0;
/// Between buttons.
pub const BUTTON_SPACE_DIP: f64 = 8.0;
pub const BUTTON_SIZE_DIP: f64 = 12.5;
pub const BUTTONS_RIGHT_DIP: f64 = TEXT_RIGHT_DIP;

/// The height of a card with `body_lines` lines of body, a thumbnail
/// `thumb_h` dip tall (0 for none) and, with `actions`, a button row.
/// Title only is 44 dip; one body line makes it 62, Toru's original size.
pub fn card_height_dip(body_lines: usize, thumb_h: f64, actions: bool) -> f64 {
    let mut h = PAD_TOP_DIP + TITLE_DIP + PAD_BOTTOM_DIP;
    let lines = body_lines.min(MAX_BODY_LINES);
    if lines > 0 {
        h += BODY_GAP_DIP + lines as f64 * BODY_LINE_DIP;
    }
    if thumb_h > 0.0 {
        h += THUMB_GAP_DIP + thumb_h;
    }
    if actions {
        h += BUTTON_GAP_DIP + BUTTON_DIP;
    }
    h
}

/// A pixel rectangle, right and bottom exclusive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rect {
    pub l: i32,
    pub t: i32,
    pub r: i32,
    pub b: i32,
}

impl Rect {
    pub fn has(&self, x: i32, y: i32) -> bool {
        x >= self.l && x < self.r && y >= self.t && y < self.b
    }
}

/// A pixel position.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

/// Dip to pixels at scale.
pub fn px(dip: f64, scale: f64) -> i32 {
    (dip * scale + 0.5) as i32
}

/// Whether corner is one of the top ones.
pub fn is_top(corner: &str) -> bool {
    corner == CORNER_TOP_RIGHT || corner == CORNER_TOP_LEFT
}

/// Whether corner is one of the left ones.
pub fn is_left(corner: &str) -> bool {
    corner == CORNER_TOP_LEFT || corner == CORNER_BOTTOM_LEFT
}

/// A card's window as drawn: its rectangle on screen in px (already
/// scaled, fractional while it moves) and its opacity, 0..1.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Pose {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub o: f64,
}

impl Pose {
    /// The pose moved by (dx, dy).
    pub fn shifted(self, dx: f64, dy: f64) -> Pose {
        Pose {
            x: self.x + dx,
            y: self.y + dy,
            ..self
        }
    }

    /// The pose at s of its size, centred across, its top (or bottom) edge
    /// kept.
    pub fn scaled(self, s: f64, keep_top: bool) -> Pose {
        let (w, h) = (self.w * s, self.h * s);
        Pose {
            x: self.x + (self.w - w) / 2.0,
            y: if keep_top {
                self.y
            } else {
                self.y + self.h - h
            },
            w,
            h,
            o: self.o,
        }
    }

    /// The whole-pixel rectangle (at least 1 x 1).
    pub fn rect(self) -> Rect {
        let l = self.x.round() as i32;
        let t = self.y.round() as i32;
        Rect {
            l,
            t,
            r: l + (self.w.round() as i32).max(1),
            b: t + (self.h.round() as i32).max(1),
        }
    }
}

/// Where one card goes in the stack.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Slot {
    pub pose: Pose,
    /// Its size as a fraction of full.
    pub scale: f64,
    /// Behind a deck's front card: drawn as a blank card body, no content,
    /// no clicks.
    pub folded: bool,
    /// Stacking order, higher in front.
    pub z: i32,
}

/// The stack's geometry in px, for [`arrange`].
#[derive(Debug, Clone, Copy)]
pub struct StackSpec<'a> {
    pub work: Rect,
    pub corner: &'a str,
    /// The card width.
    pub w: i32,
    pub gap: i32,
    pub margin: i32,
    /// Cards shown in full at rest.
    pub shown: usize,
    /// How far each card behind a deck's front peeks out.
    pub peek: i32,
    /// How many peek out.
    pub depth: usize,
    /// Each step back: this much smaller and this much more transparent.
    pub shrink: f64,
    pub fade: f64,
}

/// The stack laid out: a slot per card (oldest first) and the folding.
#[derive(Debug, Clone, PartialEq)]
pub struct Arranged {
    pub slots: Vec<Slot>,
    /// Cards folded into the deck at the corner end.
    pub lower: usize,
    /// Cards folded into the deck at the far end (open and too many to fit).
    pub upper: usize,
    /// Open and too many to fit: the index (oldest first) of the first card
    /// shown in full. `None` when everything is laid out.
    pub at: Option<usize>,
    /// The largest `at`: 0 means nothing to scroll.
    pub max_at: usize,
}

/// z of the cards in a deck (minus how far back each sits) and of the
/// cards in full (plus their place in the run, newer in front).
pub const DECK_Z: i32 = 40;
pub const SHOWN_Z: i32 = 60;

/// Lays out cards of the given heights (px, oldest first) as the "list +
/// deck" model: one column from the corner out, oldest at the corner,
/// newest farthest out.
///
/// At rest (`open` false) the newest `shown` are in full and the older ones
/// fold into a deck in the corner: the newest folded card in front at full
/// size, older ones peeking out on the corner side, `peek` px each, at most
/// `depth` visible, each step back smaller and fainter, cut to the front
/// card's height. Open, every card is in full if they all fit the work
/// area; otherwise as many as fit, with a deck at each end (the far one
/// peeking away from the corner), starting at `at` (oldest first; `None`
/// opens on the newest).
pub fn arrange(spec: &StackSpec, heights: &[i32], open: bool, at: Option<usize>) -> Arranged {
    let n = heights.len();
    let top = is_top(spec.corner);
    let x = if is_left(spec.corner) {
        spec.work.l + spec.margin
    } else {
        spec.work.r - spec.margin - spec.w
    } as f64;
    let base = if top {
        spec.work.t + spec.margin
    } else {
        spec.work.b - spec.margin
    } as f64;
    let (w, gap, peek) = (spec.w as f64, spec.gap as f64, spec.peek as f64);

    let (mut lower, mut upper, mut at_out, mut max_at) = (0, 0, None, 0);
    if !open {
        lower = n.saturating_sub(spec.shown.max(1));
    } else if n > 1 {
        let room = (spec.work.b - spec.work.t - 2 * spec.margin).max(0);
        let total: i32 = heights.iter().sum::<i32>() + spec.gap * (n as i32 - 1);
        if total > room {
            // A deck at each end takes its front card and the peeks; the
            // tallest card stands in for every row, so the window never
            // runs off the screen whichever cards it shows.
            let row = heights.iter().copied().max().unwrap_or(0) + spec.gap;
            let decks = 2 * (row + spec.depth as i32 * spec.peek);
            let cap = if row > 0 {
                (room - decks + spec.gap).div_euclid(row).max(1) as usize
            } else {
                n
            };
            let cap = cap.min(n);
            max_at = n - cap;
            let a = at.unwrap_or(max_at).min(max_at);
            lower = a;
            upper = n - a - cap;
            at_out = Some(a);
        }
    }

    // A card `off` px out from the corner, h px tall (unscaled), at scale
    // s, keeping its top or bottom edge.
    let pose = |off: f64, h: f64, s: f64, o: f64, keep_top: bool| {
        let y = if top { base + off } else { base - off - h };
        Pose { x, y, w, h, o }.scaled(s, keep_top)
    };
    let mut slots = vec![
        Slot {
            pose: Pose::default(),
            scale: 1.0,
            folded: false,
            z: 0,
        };
        n
    ];
    // Folds cards [from, to) into a deck `off` px out, `front` (an index
    // into them) in front; the others peek toward the corner or away from
    // it. Each shrinks toward the edge the front card hides, so a sliver
    // shows on the peeking side. Returns the px the deck takes.
    let fold = |slots: &mut [Slot],
                from: usize,
                to: usize,
                front: usize,
                off: f64,
                toward_corner: bool| {
        let fh = heights[from + front] as f64;
        let depth = (to - from - 1).min(spec.depth);
        // The far side of a corner deck stays put, as does the corner side
        // of a far deck.
        let keep_top = if toward_corner { !top } else { top };
        for (k, slot) in slots[from..to].iter_mut().enumerate() {
            let back = k.abs_diff(front).min(spec.depth + 1);
            let seen = back.min(depth) as f64;
            let shift = if toward_corner {
                (depth as f64 - seen) * peek
            } else {
                seen * peek
            };
            let s = (1.0 - back as f64 * spec.shrink).max(0.0);
            let o = if back > spec.depth {
                0.0
            } else {
                (1.0 - back as f64 * spec.fade).max(0.0)
            };
            let h = if back > 0 {
                fh
            } else {
                heights[from + k] as f64
            };
            *slot = Slot {
                pose: pose(off + shift, h, s, o, keep_top),
                scale: s,
                folded: back > 0,
                z: DECK_Z - back as i32,
            };
        }
        depth as f64 * peek + fh
    };

    let mut run = 0.0;
    if lower > 0 {
        run = fold(&mut slots, 0, lower, lower - 1, 0.0, true) + gap;
    }
    for (j, i) in (lower..n - upper).enumerate() {
        let h = heights[i] as f64;
        slots[i] = Slot {
            pose: pose(run, h, 1.0, 1.0, !top),
            scale: 1.0,
            folded: false,
            z: SHOWN_Z + j as i32,
        };
        run += h + gap;
    }
    if upper > 0 {
        fold(&mut slots, n - upper, n, 0, run, false);
    }
    Arranged {
        slots,
        lower,
        upper,
        at: at_out,
        max_at,
    }
}

/// The smallest rectangle around every slot (empty for none).
pub fn bounds(slots: &[Slot]) -> Rect {
    let mut it = slots.iter().map(|s| s.pose.rect());
    let Some(first) = it.next() else {
        return Rect::default();
    };
    it.fold(first, |a, r| Rect {
        l: a.l.min(r.l),
        t: a.t.min(r.t),
        r: a.r.max(r.r),
        b: a.b.max(r.b),
    })
}

/// What a click lands on: a button index (0, 1), the body or the x.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    None,
    Close,
    Body,
    Button(usize),
}

impl Part {
    /// go-toast's int: a button index, -1 body, -2 close, -3 none.
    pub fn code(self) -> i32 {
        match self {
            Part::None => -3,
            Part::Close => -2,
            Part::Body => -1,
            Part::Button(i) => i as i32,
        }
    }
}

/// Where the thumbnail (tw x th dip) goes on a card with `body_lines`
/// lines of body, in px: under the body, at the text's left edge.
pub fn thumb_rect(body_lines: usize, tw: f64, th: f64, scale: f64) -> Rect {
    let top = card_height_dip(body_lines, 0.0, false) - PAD_BOTTOM_DIP + THUMB_GAP_DIP;
    let (l, t) = (px(TEXT_LEFT_DIP, scale), px(top, scale));
    Rect {
        l,
        t,
        r: l + px(tw, scale),
        b: t + px(th, scale),
    }
}

/// Where a rounded card's stripe sits from its left edge.
pub const STRIPE_INSET_DIP: f64 = 6.0;

/// The kind-coloured stripe on a card h px tall with corner radius px
/// (both px). Square cards (Toru) run it down the whole left edge; under a
/// rounded corner that would show as a sliver, so a rounded card gets an
/// inset pill, top to bottom inside the padding (card #527). The second
/// value is the pill's corner radius (0 for the square edge).
pub fn stripe_rect(h: i32, radius: i32, scale: f64) -> (Rect, i32) {
    let w = px(STRIPE_DIP, scale);
    if radius <= 0 {
        return (
            Rect {
                l: 0,
                t: 0,
                r: w,
                b: h,
            },
            0,
        );
    }
    let l = px(STRIPE_INSET_DIP, scale);
    let t = px(PAD_TOP_DIP, scale).min(h / 2);
    let b = (h - px(PAD_BOTTOM_DIP, scale)).max(t);
    (Rect { l, t, r: l + w, b }, w / 2)
}

/// The x's hit box on a card w px wide.
pub fn close_rect(w: i32, scale: f64) -> Rect {
    let s = px(CLOSE_SIZE_DIP, scale);
    let inset = px(CLOSE_INSET_DIP, scale);
    Rect {
        l: w - inset - s,
        t: inset,
        r: w - inset,
        b: inset + s,
    }
}

/// The button row along the bottom of a card w x h px, given each label's
/// measured text width in px. Buttons read left to right in action order
/// (the primary first) and the row ends at the right padding.
pub fn button_rects(w: i32, h: i32, text_w: &[i32], scale: f64) -> Vec<Rect> {
    if text_w.is_empty() {
        return vec![];
    }
    let min = px(BUTTON_MIN_DIP, scale);
    let widths: Vec<i32> = text_w
        .iter()
        .map(|tw| (tw + 2 * px(BUTTON_PAD_DIP, scale)).max(min))
        .collect();
    let space = px(BUTTON_SPACE_DIP, scale);
    let total: i32 = widths.iter().sum::<i32>() + (widths.len() as i32 - 1) * space;
    let bottom = h - px(PAD_BOTTOM_DIP, scale);
    let top = bottom - px(BUTTON_DIP, scale);
    let mut x = (w - px(BUTTONS_RIGHT_DIP, scale) - total).max(px(TEXT_LEFT_DIP, scale));
    widths
        .iter()
        .map(|bw| {
            let r = Rect {
                l: x,
                t: top,
                r: x + bw,
                b: bottom,
            };
            x += bw + space;
            r
        })
        .collect()
}

/// What (x, y) on a card lands on.
pub fn hit_part(x: i32, y: i32, close: Rect, buttons: &[Rect]) -> Part {
    if close.has(x, y) {
        return Part::Close;
    }
    for (i, b) in buttons.iter().enumerate() {
        if b.has(x, y) {
            return Part::Button(i);
        }
    }
    Part::Body
}
