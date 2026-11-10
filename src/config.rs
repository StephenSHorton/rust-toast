//! Colors, palettes and the app-level setup (go-toast config.go).

use std::fmt;
use std::time::Duration;

/// An sRGB color made with [`rgb`] or [`hex`]. The zero `Color` is "unset"
/// (a palette fills it in), so black is `rgb(0, 0, 0)`, not `Color(0)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct Color(pub u32);

const COLOR_SET: u32 = 0xFF00_0000;

/// The color r, g, b.
pub const fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color(COLOR_SET | (r as u32) << 16 | (g as u32) << 8 | b as u32)
}

/// Parses `#rrggbb` or `rrggbb`. Panics on anything else: it is meant for
/// palette literals.
pub fn hex(s: &str) -> Color {
    let t = s.strip_prefix('#').unwrap_or(s);
    match u32::from_str_radix(t, 16) {
        Ok(v) if t.len() == 6 && t.bytes().all(|b| b.is_ascii_hexdigit()) => Color(COLOR_SET | v),
        _ => panic!("toast: bad hex color {s:?}"),
    }
}

/// [`hex`] for constants.
pub(crate) const fn hx(v: u32) -> Color {
    Color(COLOR_SET | v)
}

impl Color {
    /// Whether c is unset.
    pub fn is_zero(self) -> bool {
        self.0 & COLOR_SET == 0
    }

    /// The channels.
    pub fn rgb(self) -> (u8, u8, u8) {
        ((self.0 >> 16) as u8, (self.0 >> 8) as u8, self.0 as u8)
    }

    /// The color as a Win32 COLORREF (0x00BBGGRR).
    pub fn colorref(self) -> u32 {
        let (r, g, b) = self.rgb();
        r as u32 | (g as u32) << 8 | (b as u32) << 16
    }

    /// 0xRRGGBB, the AppKit side's form.
    pub fn rgb24(self) -> u32 {
        self.0 & 0xFF_FFFF
    }

    /// WCAG relative luminance.
    fn luminance(self) -> f64 {
        let lin = |v: u8| {
            let s = v as f64 / 255.0;
            if s <= 0.04045 {
                s / 12.92
            } else {
                ((s + 0.055) / 1.055).powf(2.4)
            }
        };
        let (r, g, b) = self.rgb();
        0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
    }
}

impl fmt::Display for Color {
    /// `#rrggbb`, or `unset`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_zero() {
            return f.write_str("unset");
        }
        write!(f, "#{:06x}", self.0 & 0xFF_FFFF)
    }
}

/// Moves a fraction t of the way from a toward b (Go rounds half away from
/// zero, as `f64::round` does).
pub fn mix(a: Color, b: Color, t: f64) -> Color {
    let (ar, ag, ab) = a.rgb();
    let (br, bg, bb) = b.rgb();
    let m = |x: u8, y: u8| (x as f64 + (y as f64 - x as f64) * t).round() as u8;
    rgb(m(ar, br), m(ag, bg), m(ab, bb))
}

/// The WCAG contrast ratio between a and b (1..21).
pub fn contrast(a: Color, b: Color) -> f64 {
    let (mut la, mut lb) = (a.luminance(), b.luminance());
    if la < lb {
        std::mem::swap(&mut la, &mut lb);
    }
    (la + 0.05) / (lb + 0.05)
}

/// A card's kind: its accent, icon and sound.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub enum Kind {
    #[default]
    Info,
    Success,
    Warn,
    Error,
}

impl Kind {
    /// The kind as go-toast's int (the AppKit view's `kind`).
    pub fn index(self) -> i32 {
        match self {
            Kind::Info => 0,
            Kind::Success => 1,
            Kind::Warn => 2,
            Kind::Error => 3,
        }
    }
}

/// The per-kind colors of a card's stripe and icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Accents {
    pub info: Color,
    pub success: Color,
    pub warn: Color,
    pub error: Color,
}

impl Accents {
    pub fn for_kind(&self, k: Kind) -> Color {
        match k {
            Kind::Success => self.success,
            Kind::Warn => self.warn,
            Kind::Error => self.error,
            Kind::Info => self.info,
        }
    }
}

/// The cards' colors. Background, foreground, muted, border and accent are
/// the ones to set; the rest are derived when left unset. Any of the five
/// left unset comes from [`DARK`], and a wholly zero palette is Dark.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Palette {
    pub background: Color,
    pub foreground: Color,
    pub muted: Color,
    pub border: Color,
    pub accent: Accents,
    /// The x on a card the cursor isn't over; default muted 40% toward background.
    pub faint: Color,
    /// Hover fill; default background 8% toward foreground.
    pub hover: Color,
    /// Secondary button fill; default background 5% toward foreground.
    pub button: Color,
    /// The first button; default accent.info.
    pub primary: Color,
    /// Default primary 15% toward foreground.
    pub primary_hover: Color,
    /// Text on the first button; default whichever of background and
    /// foreground contrasts more with primary.
    pub on_primary: Color,
    /// The glass's colour, drawn at [`Config::glass`] over the blurred
    /// desktop; default background. Unused by solid cards.
    pub tint: Color,
}

/// Toru's palette: near-black cards with a blue accent. Success is the
/// window's success green (toru.css `--success`, oklch(0.74 0.15 152), the
/// "Ready for you" pill; card #175); warn is amber, error red.
pub const DARK: Palette = Palette {
    background: hx(0x111213),
    foreground: hx(0xfafafa),
    muted: hx(0x9ea1a7),
    border: hx(0x2c2e31),
    accent: Accents {
        info: hx(0x4cabfd),
        success: hx(0x54c57a),
        warn: hx(0xf5ae39),
        error: hx(0xea3c3f),
    },
    faint: hx(0x5c5f66),
    hover: hx(0x26292e),
    button: hx(0x202225),
    primary: hx(0x4cabfd),
    primary_hover: hx(0x6fbcff),
    on_primary: hx(0x060a13),
    tint: hx(0x111213),
};

/// White cards with slate text. Pair it with a corner radius of about 10.
pub const LIGHT: Palette = Palette {
    background: hx(0xffffff),
    foreground: hx(0x0f172a),
    muted: hx(0x64748b),
    border: rgb(203, 213, 225),
    accent: Accents {
        info: hx(0x2563eb),
        success: hx(0x15803d),
        warn: hx(0xc2620a),
        error: hx(0xef4444),
    },
    faint: Color(0),
    hover: Color(0),
    button: Color(0),
    primary: Color(0),
    primary_hover: Color(0),
    on_primary: Color(0),
    tint: Color(0),
};

impl Palette {
    /// Every unset color filled.
    pub fn resolved(self) -> Palette {
        let mut p = self;
        if p == Palette::default() {
            p = DARK;
        }
        fn fill(c: &mut Color, def: Color) {
            if c.is_zero() {
                *c = def;
            }
        }
        fill(&mut p.background, DARK.background);
        fill(&mut p.foreground, DARK.foreground);
        fill(&mut p.muted, DARK.muted);
        fill(&mut p.border, DARK.border);
        fill(&mut p.accent.info, DARK.accent.info);
        fill(&mut p.accent.success, DARK.accent.success);
        fill(&mut p.accent.warn, DARK.accent.warn);
        fill(&mut p.accent.error, DARK.accent.error);
        fill(&mut p.faint, mix(p.muted, p.background, 0.4));
        fill(&mut p.hover, mix(p.background, p.foreground, 0.08));
        fill(&mut p.button, mix(p.background, p.foreground, 0.05));
        fill(&mut p.primary, p.accent.info);
        fill(&mut p.primary_hover, mix(p.primary, p.foreground, 0.15));
        let on = if contrast(p.foreground, p.primary) > contrast(p.background, p.primary) {
            p.foreground
        } else {
            p.background
        };
        fill(&mut p.on_primary, on);
        fill(&mut p.tint, p.background);
        p
    }
}

/// Which of the two palettes the cards use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub enum Scheme {
    Light,
    /// Toru's and the old default.
    #[default]
    Dark,
    /// Follows the OS's light or dark mode, flipping with it.
    System,
}

/// The app-level setup, set with [`crate::configure`]. Set it again at
/// any time: the cards on screen restyle at once (the app name and window
/// classes are fixed when the first card opens). Start from
/// [`Config::default`]: its `capture_exclude` is true.
///
/// Not ported: go-toast's `IgnoreInput` and `HoldClickThrough` (Toru's
/// auto clicker).
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// The windows' title and the log prefix (`<app>: toast: ...`).
    pub app_name: String,
    /// The window classes: the cards are `class_prefix`, the hidden host
    /// `class_prefix + "Host"`. Empty derives it from the app name.
    pub class_prefix: String,
    /// The light palette (a zero one is [`LIGHT`]).
    pub light: Palette,
    /// The dark palette (a zero one is [`DARK`]).
    pub dark: Palette,
    /// Which palette shows.
    pub scheme: Scheme,
    /// In dip; 0 is square (Toru).
    pub corner_radius: f64,
    /// Real glass where the OS has it (macOS NSVisualEffectView, Windows
    /// acrylic): the palette's `tint` at this opacity over the blurred
    /// desktop. 1 (or more), NaN or 0 is no glass: the cards are a solid
    /// `background`, which is also the fallback where there is no blur.
    pub glass: f64,
    /// Keeps the cards out of screenshots and recordings
    /// (WDA_EXCLUDEFROMCAPTURE, NSWindowSharingNone).
    pub capture_exclude: bool,
    /// The kind-coloured bar down the card's left edge. Off, the icon alone
    /// carries the kind's colour.
    pub stripe: bool,
    /// How the stack folds, opens and moves ([`Motion::default`] is the
    /// designed feel).
    pub motion: Motion,
}

impl Default for Config {
    /// App name "Toast", Dark, square, solid, excluded from capture, with
    /// the stripe.
    fn default() -> Self {
        Config {
            app_name: "Toast".into(),
            class_prefix: String::new(),
            light: LIGHT,
            dark: DARK,
            scheme: Scheme::Dark,
            corner_radius: 0.0,
            glass: 1.0,
            capture_exclude: true,
            stripe: true,
            motion: Motion::default(),
        }
    }
}

pub const MAX_CORNER_RADIUS: f64 = 24.0;

impl Config {
    /// Defaults filled and out-of-range values clamped.
    pub fn normalized(mut self) -> Config {
        self.app_name = self.app_name.trim().to_string();
        if self.app_name.is_empty() {
            self.app_name = Config::default().app_name;
        }
        if self.class_prefix.is_empty() {
            self.class_prefix = class_prefix_for(&self.app_name);
        }
        self.light = if self.light == Palette::default() {
            LIGHT.resolved()
        } else {
            self.light.resolved()
        };
        self.dark = self.dark.resolved();
        if self.corner_radius.is_nan() || self.corner_radius < 0.0 {
            self.corner_radius = 0.0;
        } else if self.corner_radius > MAX_CORNER_RADIUS {
            self.corner_radius = MAX_CORNER_RADIUS;
        }
        if !(self.glass > 0.0 && self.glass < 1.0) {
            self.glass = 1.0;
        }
        self.motion = self.motion.normalized();
        self
    }

    /// The palette for a light (false) or dark (true) look.
    pub fn palette(&self, dark: bool) -> Palette {
        if dark { self.dark } else { self.light }
    }

    /// Whether the cards are dark, given whether the OS is.
    pub fn is_dark(&self, os_dark: bool) -> bool {
        match self.scheme {
            Scheme::Light => false,
            Scheme::Dark => true,
            Scheme::System => os_dark,
        }
    }

    /// The glass tint's opacity, or `None` for solid cards.
    pub fn glass_alpha(&self) -> Option<f64> {
        (self.glass > 0.0 && self.glass < 1.0).then_some(self.glass)
    }

    pub fn host_class_name(&self) -> String {
        format!("{}Host", self.class_prefix)
    }

    pub fn card_class_name(&self) -> String {
        self.class_prefix.clone()
    }

    /// `<app>: toast: `, lowercased.
    pub fn log_prefix(&self) -> String {
        let a = self.app_name.to_lowercase();
        if a == "toast" {
            return "toast: ".into();
        }
        format!("{a}: toast: ")
    }
}

/// A cubic-bezier easing curve, as CSS writes it: `[x1, y1, x2, y2]`.
pub type Ease = [f64; 4];

/// How the stack moves: the "list + deck" model. The cards run in one
/// column from the corner out, oldest at the corner and newest farthest
/// out. At rest the newest [`shown`](Motion::shown) are in full and the
/// older ones fold into a deck that stays in the corner. Hovering opens the
/// stack (with a deck at each end and the wheel or a vertical drag moving
/// through when it doesn't fit the screen); leaving folds it again. A card
/// dragged sideways is thrown away.
///
/// Distances are in dip, speeds in dip per millisecond, fractions 0..1.
/// The OS's reduce-motion setting turns every animation into a jump.
#[derive(Debug, Clone, PartialEq)]
pub struct Motion {
    /// Cards shown in full at rest; older ones fold into the corner deck.
    pub shown: usize,
    /// The most cards up at once: a post past it pushes the oldest
    /// non-sticky card off.
    pub max_cards: usize,
    /// How far each card behind a deck's front card peeks out.
    pub peek: f64,
    /// How many cards peek out behind a deck's front card.
    pub peek_depth: usize,
    /// Each step back in a deck is this much smaller (a fraction of full)...
    pub peek_shrink: f64,
    /// ...and this much more transparent.
    pub peek_fade: f64,
    /// The drag distance that decides a drag's axis, once, for the whole drag.
    pub lock: f64,
    /// Released past this far sideways, a dragged card is thrown away.
    pub dismiss_distance: f64,
    /// Released faster than this sideways, it is thrown away regardless.
    pub flick_speed: f64,
    /// A card dragged sideways fades over this distance...
    pub drag_fade: f64,
    /// ...down to this opacity.
    pub drag_min_opacity: f64,
    /// Vertical drag per card scrolled, while the open stack has hidden cards.
    pub scroll_step: f64,
    /// A vertical drag with nothing to scroll moves the card this fraction
    /// of the drag, then springs back.
    pub rubber_band: f64,
    /// Wheel notches closer together than this move one card, not several.
    pub wheel_debounce: Duration,
    /// After a card is thrown away, leaving the stack folds it only this
    /// long after the throw (time to come back for the next one).
    pub after_throw: Duration,
    /// Every move, fold, open and fade.
    pub duration: Duration,
    /// A card leaving takes this fraction of `duration`.
    pub exit: f64,
    /// Position and size: fast out, a slight overshoot, soft landing.
    pub ease: Ease,
    /// Opacity and leaving cards: no overshoot.
    pub ease_out: Ease,
    /// A new card slides in from this far toward the screen edge...
    pub enter_offset: f64,
    /// ...from this size (a fraction of full), and transparent.
    pub enter_scale: f64,
    /// A card closing slides this far toward the edge as it fades.
    pub leave_offset: f64,
    /// A thrown card flies this far.
    pub throw_offset: f64,
}

impl Default for Motion {
    /// The feel designed in the motion lab: 3 shown, at most 5 up, 12 dip
    /// peeks two deep, 420 ms springy moves.
    fn default() -> Self {
        Motion {
            shown: 3,
            max_cards: 5,
            peek: 12.0,
            peek_depth: 2,
            peek_shrink: 0.05,
            peek_fade: 0.22,
            lock: 12.0,
            dismiss_distance: 110.0,
            flick_speed: 0.6,
            drag_fade: 260.0,
            drag_min_opacity: 0.2,
            scroll_step: 44.0,
            rubber_band: 0.15,
            wheel_debounce: Duration::from_millis(90),
            after_throw: Duration::from_millis(1200),
            duration: Duration::from_millis(420),
            exit: 0.8,
            ease: [0.2, 0.9, 0.25, 1.15],
            ease_out: [0.2, 0.8, 0.2, 1.0],
            enter_offset: 60.0,
            enter_scale: 0.96,
            leave_offset: 40.0,
            throw_offset: 420.0,
        }
    }
}

/// The most cards a deck shows peeking behind its front.
pub const MAX_PEEK_DEPTH: usize = 8;

impl Motion {
    /// Snaps hand-edited values to usable ones: counts at least 1, NaN or
    /// negative distances to their defaults, fractions into 0..1, easing
    /// x values into 0..1.
    pub fn normalized(mut self) -> Motion {
        let d = Motion::default();
        self.shown = self.shown.max(1);
        self.max_cards = self.max_cards.max(1);
        self.peek_depth = self.peek_depth.min(MAX_PEEK_DEPTH);
        let dist = |v: &mut f64, def: f64| {
            if !v.is_finite() || *v < 0.0 {
                *v = def;
            }
        };
        dist(&mut self.peek, d.peek);
        dist(&mut self.lock, d.lock);
        dist(&mut self.dismiss_distance, d.dismiss_distance);
        dist(&mut self.flick_speed, d.flick_speed);
        dist(&mut self.scroll_step, d.scroll_step);
        dist(&mut self.enter_offset, d.enter_offset);
        dist(&mut self.leave_offset, d.leave_offset);
        dist(&mut self.throw_offset, d.throw_offset);
        dist(&mut self.drag_fade, d.drag_fade);
        if self.drag_fade == 0.0 {
            self.drag_fade = d.drag_fade;
        }
        if self.scroll_step == 0.0 {
            self.scroll_step = d.scroll_step;
        }
        let frac = |v: &mut f64, def: f64| {
            if v.is_finite() {
                *v = v.clamp(0.0, 1.0);
            } else {
                *v = def;
            }
        };
        frac(&mut self.peek_shrink, d.peek_shrink);
        frac(&mut self.peek_fade, d.peek_fade);
        frac(&mut self.drag_min_opacity, d.drag_min_opacity);
        frac(&mut self.rubber_band, d.rubber_band);
        frac(&mut self.exit, d.exit);
        frac(&mut self.enter_scale, d.enter_scale);
        for (e, def) in [(&mut self.ease, d.ease), (&mut self.ease_out, d.ease_out)] {
            if e.iter().any(|v| !v.is_finite()) {
                *e = def;
            }
            e[0] = e[0].clamp(0.0, 1.0);
            e[2] = e[2].clamp(0.0, 1.0);
        }
        self
    }
}

/// A window class prefix from an app name: its ASCII letters and digits
/// plus `Toast`.
pub fn class_prefix_for(app: &str) -> String {
    let mut s: String = app.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    if !s.ends_with("Toast") {
        s.push_str("Toast");
    }
    s
}
