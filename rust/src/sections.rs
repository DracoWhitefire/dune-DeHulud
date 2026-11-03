//! The window's sections and what the damage sections show: the three switches, the layout (section order and
//! heights), the header's click zones and click-or-drag rule, and the damage numbers by poll age (order, at most
//! four per column, fading, removal, the five-digit cap, `-`). Pure functions, the same as in `dehulud.py`; the
//! drawing is in [`crate::window`].

/// The window's width; the height follows the sections that are on.
pub const W: i32 = 180;
/// Section heights in pixels, top to bottom.
pub const HEADER_H: i32 = 18;
pub const WORM_H: i32 = 24;
pub const TAKEN_H: i32 = 60;
pub const DEALT_H: i32 = 76;
pub const STATUS_H: i32 = 20;
/// The tallest window (every section on).
pub const MAX_H: i32 = HEADER_H + WORM_H + TAKEN_H + DEALT_H + STATUS_H;
/// A press that moves this far (in x or y) by its release is a drag, not a click.
pub const CLICK_PX: i32 = 3;
/// A number is removed on its fifth poll (shown at ages 0 to 4).
pub const LIFE: u32 = 5;
/// Numbers per column.
pub const ROWS: usize = 4;
/// At most five digits.
pub const CAP: u32 = 99999;

pub const LABEL: u32 = 0xd8cfb4;
pub const OFF: u32 = 0x6b7174;
pub const DASH: u32 = 0x85969d;
pub const RED: u32 = 0xee7868;
pub const AMBER: u32 = 0xe8bf70;
pub const TEAL: u32 = 0x8ad4c8;
/// Colour by age in polls: full at ages 0 and 1, then blended towards the background at 70, 45 and 20 %.
pub const FADE: [(u32, [u32; 5]); 3] = [
    (RED, [RED, RED, 0xaf5d53, 0x7b4741, 0x47312f]),
    (AMBER, [AMBER, AMBER, 0xab8f58, 0x786745, 0x463f31]),
    (TEAL, [TEAL, TEAL, 0x699e96, 0x4e706c, 0x334342]),
];
/// Top of the big glyphs per row in the Damage taken section (row 1, the newest, first).
pub const TAKEN_TOPS: [i32; 4] = [7, 20, 33, 46];
/// Top of the big glyphs per row in the Damage dealt section (row 4 is the newest).
pub const DEALT_TOPS: [i32; 4] = [22, 35, 48, 61];

/// One of the window's three switchable sections.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Feature {
    Worm,
    Taken,
    Dealt,
}

impl Feature {
    pub const ALL: [Feature; 3] = [Feature::Worm, Feature::Taken, Feature::Dealt];

    /// The config file key (also the name the Python version uses).
    pub fn key(self) -> &'static str {
        match self {
            Feature::Worm => "worm",
            Feature::Taken => "damage_taken",
            Feature::Dealt => "damage_dealt",
        }
    }

    /// The command-line flag that switches it off for one run.
    pub fn flag(self) -> &'static str {
        match self {
            Feature::Worm => "--no-worm",
            Feature::Taken => "--no-taken",
            Feature::Dealt => "--no-dealt",
        }
    }

    /// Its header switch: click zone x0..=x1, x of the text, label.
    pub fn zone(self) -> (i32, i32, i32, &'static str) {
        match self {
            Feature::Worm => (0, 56, 6, "Worm"),
            Feature::Taken => (57, 116, 60, "Taken"),
            Feature::Dealt => (117, 179, 120, "Dealt"),
        }
    }
}

/// A set of features: the sections that are on (or, for flags, the ones switched off).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Features {
    pub worm: bool,
    pub taken: bool,
    pub dealt: bool,
}

impl Features {
    pub const ALL: Features = Features { worm: true, taken: true, dealt: true };
    pub const NONE: Features = Features { worm: false, taken: false, dealt: false };
    /// What the terminal line shows (the section flags are for the window).
    pub const TEXT: Features = Features { worm: true, taken: false, dealt: false };

    pub fn has(self, f: Feature) -> bool {
        match f {
            Feature::Worm => self.worm,
            Feature::Taken => self.taken,
            Feature::Dealt => self.dealt,
        }
    }

    pub fn set(&mut self, f: Feature, on: bool) {
        match f {
            Feature::Worm => self.worm = on,
            Feature::Taken => self.taken = on,
            Feature::Dealt => self.dealt = on,
        }
    }

    pub fn any(self) -> bool {
        self.worm || self.taken || self.dealt
    }

    /// The config keys of the features in the set, in header order.
    pub fn keys(self) -> Vec<&'static str> {
        Feature::ALL.iter().filter(|f| self.has(**f)).map(|f| f.key()).collect()
    }
}

/// The window's sections, top to bottom.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    Header,
    Worm,
    Taken,
    Dealt,
    Status,
}

impl Section {
    /// The name the Python version uses for it.
    pub fn name(self) -> &'static str {
        match self {
            Section::Header => "header",
            Section::Worm => "worm",
            Section::Taken => "damage_taken",
            Section::Dealt => "damage_dealt",
            Section::Status => "status",
        }
    }
}

/// The sections shown with their top y, and the window height. The header is always shown, the status line
/// whenever at least one section is on; the order never changes.
pub fn layout(on: Features) -> (Vec<(Section, i32)>, i32) {
    let all = [
        (Section::Header, HEADER_H, true),
        (Section::Worm, WORM_H, on.worm),
        (Section::Taken, TAKEN_H, on.taken),
        (Section::Dealt, DEALT_H, on.dealt),
        (Section::Status, STATUS_H, on.any()),
    ];
    let mut y = 0;
    let mut out = Vec::new();
    for (s, h, shown) in all {
        if shown {
            out.push((s, y));
            y += h;
        }
    }
    (out, y)
}

/// The feature whose header switch is at window pixel (x, y), or None outside the header row.
pub fn header_zone(x: i32, y: i32) -> Option<Feature> {
    if !(0..HEADER_H).contains(&y) {
        return None;
    }
    Feature::ALL.into_iter().find(|f| {
        let (x0, x1, _, _) = f.zone();
        x0 <= x && x <= x1
    })
}

/// Whether a press that moved (dx, dy) by its release is a click (else it is a drag).
pub fn is_click(dx: i32, dy: i32) -> bool {
    dx.abs() < CLICK_PX && dy.abs() < CLICK_PX
}

/// The header row: x, text and colour of the three switches.
pub fn header_cells(on: Features) -> Vec<(i32, String, u32)> {
    Feature::ALL
        .iter()
        .map(|f| {
            let (_, _, x, label) = f.zone();
            if on.has(*f) { (x, format!("[x] {label}"), LABEL) } else { (x, format!("[ ] {label}"), OFF) }
        })
        .collect()
}

/// The whole points shown for an emitted drop (capped at five digits), or None for nothing (a drop that rounds
/// to 0, or a value that is not a non-negative number).
pub fn shown_number(v: f64) -> Option<u32> {
    if !v.is_finite() || v < 0.0 {
        return None;
    }
    let n = (v + 0.5).floor();
    if n <= 0.0 { None } else { Some(n.min(f64::from(CAP)) as u32) }
}

/// The colour of a number of base colour `rgb` at `age` polls (None once it is removed).
pub fn fade(rgb: u32, age: u32) -> Option<u32> {
    let steps = FADE.iter().find(|(b, _)| *b == rgb)?.1;
    steps.get(age as usize).copied()
}

/// One column of damage numbers, newest first, each with its age in polls; or `-` after an unreadable poll.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Column {
    pub items: Vec<(u32, u32)>,
    pub dash: bool,
}

impl Default for Column {
    fn default() -> Self {
        Column { items: Vec::new(), dash: true } // nothing read yet: '-'
    }
}

impl Column {
    /// One poll: None = nothing trustworthy (clear, `-`), else the number emitted in this poll, if any.
    pub fn step(&mut self, emitted: Option<Option<f64>>) {
        let Some(emitted) = emitted else {
            self.items.clear();
            self.dash = true;
            return;
        };
        self.dash = false;
        self.items = self.items.iter().filter(|(_, age)| age + 1 < LIFE).map(|(n, age)| (*n, age + 1)).collect();
        if let Some(n) = emitted.and_then(shown_number) {
            self.items.insert(0, (n, 0));
        }
        self.items.truncate(ROWS);
    }
}

/// What the two damage sections show: the Damage taken column and the Damage dealt health and shield columns.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DamageView {
    pub taken: Column,
    pub health: Column,
    pub shield: Column,
}

impl DamageView {
    /// Forgets a damage section's numbers (its switch was flipped): it starts again with `-`.
    pub fn reset(&mut self, f: Feature) {
        match f {
            Feature::Worm => {}
            Feature::Taken => self.taken = Column::default(),
            Feature::Dealt => {
                self.health = Column::default();
                self.shield = Column::default();
            }
        }
    }

    /// One poll's damage values. A section that is off forgets its numbers (it starts with `-` when switched on).
    pub fn step(&mut self, taken: Option<Option<f64>>, dealt: Option<(Option<f64>, Option<f64>)>, on: Features) {
        if on.taken {
            self.taken.step(taken);
        } else {
            self.taken = Column::default();
        }
        if on.dealt {
            self.health.step(dealt.map(|d| d.0));
            self.shield.step(dealt.map(|d| d.1));
        } else {
            self.health = Column::default();
            self.shield = Column::default();
        }
    }
}

/// One number to draw: section, right x, top y inside the section, text, colour (big font, right-aligned).
pub type Cell = (Section, i32, i32, String, u32);

/// The numbers to draw for the damage sections that are on.
pub fn damage_cells(v: &DamageView, on: Features) -> Vec<Cell> {
    let mut out = Vec::new();
    let mut column = |section: Section, col: &Column, right: i32, tops: &[i32; 4], rgb: u32, sign: &str| {
        let top = |i: usize| if section == Section::Taken { tops[i] } else { tops[ROWS - 1 - i] };
        if col.dash {
            out.push((section, right, top(0), "-".to_string(), DASH));
            return;
        }
        for (i, (n, age)) in col.items.iter().enumerate() {
            out.push((section, right, top(i), format!("{sign}{n}"), fade(rgb, *age).unwrap_or(rgb)));
        }
    };
    if on.taken {
        column(Section::Taken, &v.taken, 174, &TAKEN_TOPS, RED, "-");
    }
    if on.dealt {
        column(Section::Dealt, &v.health, 122, &DEALT_TOPS, AMBER, "");
        column(Section::Dealt, &v.shield, 174, &DEALT_TOPS, TEAL, "");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feats(worm: bool, taken: bool, dealt: bool) -> Features {
        Features { worm, taken, dealt }
    }

    #[test]
    fn heights_of_all_eight_combinations() {
        let want = [
            ((false, false, false), 18),
            ((true, false, false), 62),
            ((false, true, false), 98),
            ((false, false, true), 114),
            ((true, true, false), 122),
            ((true, false, true), 138),
            ((false, true, true), 174),
            ((true, true, true), 198),
        ];
        for ((w, t, d), h) in want {
            assert_eq!(layout(feats(w, t, d)).1, h, "{w} {t} {d}");
        }
        assert_eq!(MAX_H, 198);
        assert_eq!(
            layout(Features::ALL).0,
            [
                (Section::Header, 0),
                (Section::Worm, 18),
                (Section::Taken, 42),
                (Section::Dealt, 102),
                (Section::Status, 178)
            ]
        );
        assert_eq!(layout(Features::NONE).0, [(Section::Header, 0)]);
    }

    #[test]
    fn header_zones_and_clicks() {
        let z: Vec<_> = [0, 56, 57, 116, 117, 179].iter().map(|x| header_zone(*x, 9)).collect();
        use Feature::*;
        assert_eq!(z, [Some(Worm), Some(Worm), Some(Taken), Some(Taken), Some(Dealt), Some(Dealt)]);
        assert_eq!([header_zone(30, 0), header_zone(30, 17)], [Some(Worm), Some(Worm)]);
        assert_eq!([header_zone(30, 18), header_zone(30, -1), header_zone(-1, 5), header_zone(180, 5)], [None; 4]);
        assert!(is_click(2, -2) && is_click(0, 0) && !is_click(3, 0) && !is_click(0, -3));
    }

    #[test]
    fn header_strings_fit_their_zones() {
        for f in Feature::ALL {
            let (x0, x1, x, label) = f.zone();
            let w = crate::font::SMALL.width(&format!("[x] {label}"), false);
            assert!(x0 <= x && x + w <= x1 + 1, "{label}");
        }
        assert_eq!(header_cells(feats(false, true, false))[0], (6, "[ ] Worm".to_string(), OFF));
    }

    #[test]
    fn rounding_cap_and_fades() {
        let got: Vec<_> = [0.0, 0.49, 0.5, 1.5, 2.5, 36.6, 99999.4, 99999.5, 1e12, -1.0, f64::NAN, f64::INFINITY]
            .iter()
            .map(|v| shown_number(*v))
            .collect();
        let want = [None, None, Some(1), Some(2), Some(3), Some(37), Some(99999), Some(99999), Some(99999)];
        assert_eq!(got[..9], want);
        assert_eq!(got[9..], [None; 3]);
        // the dim steps are the colour blended towards the background at 70, 45 and 20 %
        let blend = |rgb: u32, k: f64| {
            let ch = |s: u32, bg: u32| {
                let (c, b) = (f64::from((rgb >> s) & 0xff), f64::from(bg));
                ((b + (c - b) * k / 100.0).round() as u32) << s
            };
            ch(16, 0x1d) | ch(8, 0x1f) | ch(0, 0x21)
        };
        for (base, steps) in FADE {
            assert_eq!(steps[2..], [blend(base, 70.0), blend(base, 45.0), blend(base, 20.0)]);
        }
        assert_eq!(fade(RED, 5), None);
    }

    #[test]
    fn column_order_life_and_dash() {
        let mut c = Column::default();
        assert!(c.dash);
        for v in 1..=5 {
            c.step(Some(Some(f64::from(v))));
        }
        assert_eq!(c.items, [(5, 0), (4, 1), (3, 2), (2, 3)]);
        let mut c = Column::default();
        c.step(Some(Some(37.0)));
        let mut ages = Vec::new();
        for _ in 0..6 {
            ages.push(c.items.iter().map(|i| i.1).collect::<Vec<_>>());
            c.step(Some(None));
        }
        assert_eq!(ages, [vec![0], vec![1], vec![2], vec![3], vec![4], vec![]]);
        c.step(Some(Some(12.0)));
        c.step(None);
        assert_eq!((c.dash, c.items.len()), (true, 0));
        c.step(Some(Some(0.2)));
        assert_eq!((c.dash, c.items.len()), (false, 0));
    }

    #[test]
    fn cells() {
        let mut v = DamageView::default();
        v.step(Some(Some(37.0)), Some((Some(120.0), None)), Features::ALL);
        v.step(Some(Some(12.0)), Some((Some(84.0), Some(40.0))), Features::ALL);
        v.step(Some(None), Some((None, None)), Features::ALL);
        let c = |s, x, y, t: &str, rgb| (s, x, y, t.to_string(), rgb);
        assert_eq!(
            damage_cells(&v, Features::ALL),
            [
                c(Section::Taken, 174, 7, "-12", RED),
                c(Section::Taken, 174, 20, "-37", 0xaf5d53),
                c(Section::Dealt, 122, 61, "84", AMBER),
                c(Section::Dealt, 122, 48, "120", 0xab8f58),
                c(Section::Dealt, 174, 61, "40", TEAL),
            ]
        );
        v.step(None, None, Features::ALL); // unreadable, or an unsupported build
        assert_eq!(
            damage_cells(&v, Features::ALL),
            [
                c(Section::Taken, 174, 7, "-", DASH),
                c(Section::Dealt, 122, 61, "-", DASH),
                c(Section::Dealt, 174, 61, "-", DASH)
            ]
        );
        let mut v = DamageView::default();
        v.step(Some(Some(123456.0)), Some((Some(1e9), Some(99999.4))), Features::ALL);
        let texts: Vec<String> = damage_cells(&v, Features::ALL).into_iter().map(|c| c.3).collect();
        assert_eq!(texts, ["-99999", "99999", "99999"]);
        assert!(174 - crate::font::BIG.width("-99999", false) >= 129, "the widest number fits its column");
        assert!(122 - crate::font::BIG.width("99999", false) >= 83);
    }

    #[test]
    fn a_section_switched_off_forgets_its_numbers() {
        let mut v = DamageView::default();
        v.step(Some(Some(5.0)), Some((Some(6.0), None)), Features::ALL);
        let no_taken = feats(true, false, true);
        v.step(Some(None), Some((None, None)), no_taken);
        assert_eq!(damage_cells(&v, no_taken), [(Section::Dealt, 122, 61, "6".to_string(), AMBER)]);
        assert_eq!(damage_cells(&v, Features::ALL)[0], (Section::Taken, 174, 7, "-".to_string(), DASH));
    }
}
