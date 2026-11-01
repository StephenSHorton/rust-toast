//! The confetti burst a success card throws when the person turns it on
//! (Settings, "Confetti on good news"): an OS-agnostic simulation the
//! platforms draw frame by frame in a click-through overlay.
//!
//! A burst is closed-form (drag and gravity solved exactly), so a frame
//! depends only on its time, never on how often the platform redraws.

use std::time::Duration;

use crate::config::{Color, hx};
use crate::layout::Rect;

/// How long a burst lasts; pieces fade out over the last [`FADE`].
pub const DURATION: Duration = Duration::from_millis(1500);
const FADE: f64 = 0.45;

/// Pieces in one burst.
pub const PIECES: usize = 150;

/// Air drag, 1/s.
const DRAG: f64 = 1.6;

const COLORS: [Color; 8] = [
    hx(0x4cabfd),
    hx(0xf5ae39),
    hx(0xea3c3f),
    hx(0x34c759),
    hx(0xaf52de),
    hx(0xffd60a),
    hx(0xff6fae),
    hx(0xfafafa),
];

/// One piece of confetti in one frame, in px from the overlay's top-left.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Piece {
    /// The centre.
    pub x: f64,
    pub y: f64,
    /// Half its width and height (the width shrinks as it flips).
    pub hw: f64,
    pub hh: f64,
    /// Radians, clockwise on screen.
    pub angle: f64,
    pub color: Color,
    /// 0..1.
    pub alpha: f64,
}

impl Piece {
    /// Its corners, in order around it.
    pub fn corners(&self) -> [(f64, f64); 4] {
        let (s, c) = self.angle.sin_cos();
        let pt = |dx: f64, dy: f64| (self.x + dx * c - dy * s, self.y + dx * s + dy * c);
        [
            pt(-self.hw, -self.hh),
            pt(self.hw, -self.hh),
            pt(self.hw, self.hh),
            pt(-self.hw, self.hh),
        ]
    }
}

#[derive(Debug, Clone)]
struct Seed {
    x: f64,
    y: f64,
    vx: f64,
    vy: f64,
    angle: f64,
    spin: f64,
    flip: f64,
    flip_rate: f64,
    hw: f64,
    hh: f64,
    color: Color,
}

/// One burst: pieces leave the card toward the middle of the screen,
/// fanned out and tossed a little upward, then drift down and fade.
#[derive(Debug, Clone)]
pub struct Burst {
    seeds: Vec<Seed>,
    gravity: f64,
}

/// A small deterministic generator (xorshift64*).
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Rng {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }
    /// 0..1.
    fn f(&mut self) -> f64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        (self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 / (1u64 << 53) as f64
    }
    fn range(&mut self, a: f64, b: f64) -> f64 {
        a + (b - a) * self.f()
    }
}

impl Burst {
    /// A burst from `card` over `area` (both px, the same top-down space);
    /// the frames come out relative to `area`'s top-left. `scale` is px per
    /// dip (piece size); `seed` varies the pattern.
    pub fn new(card: Rect, area: Rect, scale: f64, seed: u64) -> Burst {
        let mut r = Rng::new(seed);
        let (w, h) = ((area.r - area.l) as f64, (area.b - area.t) as f64);
        let diag = (w * w + h * h).sqrt().max(1.0);
        let cx = (card.l + card.r) as f64 / 2.0 - area.l as f64;
        let cy = (card.t + card.b) as f64 / 2.0 - area.t as f64;
        let (cw, ch) = ((card.r - card.l) as f64, (card.b - card.t) as f64);
        // Toward the middle of the screen, tipped upward a little.
        let mut aim = (h / 2.0 - cy).atan2(w / 2.0 - cx);
        aim -= 0.35 * aim.cos().signum();
        let seeds = (0..PIECES)
            .map(|i| {
                let a = aim + r.range(-1.2, 1.2);
                let v = diag * r.range(0.45, 1.35);
                let long = scale * r.range(4.0, 7.0);
                Seed {
                    x: cx + r.range(-0.45, 0.45) * cw,
                    y: cy + r.range(-0.4, 0.4) * ch,
                    vx: v * a.cos(),
                    vy: v * a.sin(),
                    angle: r.range(0.0, std::f64::consts::TAU),
                    spin: r.range(-9.0, 9.0),
                    flip: r.range(0.0, std::f64::consts::TAU),
                    flip_rate: r.range(6.0, 16.0),
                    hw: long,
                    hh: long * r.range(0.35, 0.6),
                    color: COLORS[i % COLORS.len()],
                }
            })
            .collect();
        Burst {
            seeds,
            gravity: diag * 0.4,
        }
    }

    /// The pieces t into the burst; empty once it is over.
    pub fn at(&self, t: Duration) -> Vec<Piece> {
        let t = t.as_secs_f64();
        let end = DURATION.as_secs_f64();
        if t >= end {
            return vec![];
        }
        let alpha = ((end - t) / FADE).min(1.0);
        let k = DRAG;
        let e = (1.0 - (-k * t).exp()) / k; // ∫ e^{-kτ} dτ over 0..t
        let g = self.gravity;
        self.seeds
            .iter()
            .map(|s| Piece {
                x: s.x + s.vx * e,
                // v(t) = (v0 - g/k) e^{-kt} + g/k
                y: s.y + (s.vy - g / k) * e + g / k * t,
                hw: s.hw * (s.flip + s.flip_rate * t).cos().abs().max(0.15),
                hh: s.hh,
                angle: s.angle + s.spin * t,
                color: s.color,
                alpha,
            })
            .collect()
    }
}

/// Draws pieces into a premultiplied BGRA buffer of w×h px (Win32's
/// layered-window format), clearing it first. A plain scanline fill: the
/// pieces are small, so no antialiasing.
pub fn rasterize(pieces: &[Piece], w: usize, h: usize, buf: &mut [u32]) {
    buf.fill(0);
    for p in pieces {
        if p.alpha <= 0.0 {
            continue;
        }
        let c = p.corners();
        let ymin = c
            .iter()
            .map(|q| q.1)
            .fold(f64::MAX, f64::min)
            .floor()
            .max(0.0) as usize;
        let ymax = c.iter().map(|q| q.1).fold(f64::MIN, f64::max).ceil();
        if ymax < 0.0 {
            continue;
        }
        let ymax = (ymax as usize).min(h);
        let a = (p.alpha.clamp(0.0, 1.0) * 255.0).round() as u32;
        let (r, g, b) = p.color.rgb();
        let pm = |v: u8| v as u32 * a / 255;
        let px = a << 24 | pm(r) << 16 | pm(g) << 8 | pm(b);
        for y in ymin..ymax {
            let yc = y as f64 + 0.5;
            let (mut lo, mut hi) = (f64::MAX, f64::MIN);
            for i in 0..4 {
                let (x0, y0) = c[i];
                let (x1, y1) = c[(i + 1) % 4];
                if (y0 <= yc) != (y1 <= yc) {
                    let x = x0 + (yc - y0) / (y1 - y0) * (x1 - x0);
                    lo = lo.min(x);
                    hi = hi.max(x);
                }
            }
            if lo > hi {
                continue;
            }
            let x0 = (lo + 0.5).floor().max(0.0) as usize;
            let x1 = ((hi + 0.5).floor().max(0.0) as usize).min(w);
            for x in x0..x1 {
                buf[y * w + x] = px;
            }
        }
    }
}
