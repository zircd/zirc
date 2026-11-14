//! zirc's look, the same in every app: colours by role for dark and light, type, the design
//! system's scales ([`Design`]), and the font files, Inter for the UI and JetBrains Mono for
//! code. It's declared once, in `tokens.json`, and everything else derives from it:
//! [`Theme`], [`Type`] and [`Design`] here, the web stylesheet ([`css`], checked in as
//! `zirc-theme.css`), and the iOS and Android apps' files, which their `theme_files.rs`
//! write from these.
//!
//! The fonts ship as files ([`fonts`]), so no app or page fetches them from anyone else.

use std::fmt::Write as _;
use std::sync::LazyLock;

use serde::Deserialize;

/// The tokens everything derives from.
pub const TOKENS: &str = include_str!("../tokens.json");

/// The web stylesheet as checked in, which [`css`] generates.
pub const CSS: &str = include_str!("../zirc-theme.css");

/// JetBrains Mono, the `mono` role, and Inter, the `ui` role, both under the SIL Open Font
/// License ([`fonts::LICENCE`], [`fonts::UI_LICENCE`]).
pub mod fonts {
    /// Upright, every weight: a variable font.
    pub const MONO: &[u8] = include_bytes!("../fonts/JetBrainsMono-Variable.ttf");
    /// Italic, every weight: a variable font.
    pub const MONO_ITALIC: &[u8] = include_bytes!("../fonts/JetBrainsMono-Italic-Variable.ttf");
    /// Upright and italic, one file per weight, for text systems that pick a weight by
    /// choosing a file rather than varying a variable font, as GPUI does: regular, bold
    /// and italic. JetBrains Mono 2.304, as the variable fonts are.
    pub const STATIC: [&[u8]; 3] = [
        include_bytes!("../fonts/JetBrainsMono-Regular.ttf"),
        include_bytes!("../fonts/JetBrainsMono-Bold.ttf"),
        include_bytes!("../fonts/JetBrainsMono-Italic.ttf"),
    ];
    /// The licence, which goes wherever the fonts do.
    pub const LICENCE: &str = include_str!("../fonts/OFL.txt");
    /// The web's fonts, in `fonts/` beside the stylesheet: file, weight, style and the TTF
    /// whose characters it has (Medium's are the variable font's, as it has no static TTF).
    pub const WEB: [(&str, u16, &str, &[u8]); 4] = [
        ("JetBrainsMono-Regular.woff2", 400, "normal", STATIC[0]),
        ("JetBrainsMono-Medium.woff2", 500, "normal", MONO),
        ("JetBrainsMono-Bold.woff2", 700, "normal", STATIC[1]),
        ("JetBrainsMono-Italic.woff2", 400, "italic", STATIC[2]),
    ];
    /// Inter 4.1, the `ui` role ([`crate::Design::ui_family`]), one file per weight, as
    /// GPUI picks them: regular, medium, semibold, bold and italic. The phones use their
    /// system font instead, at the same sizes.
    pub const UI_STATIC: [&[u8]; 5] = [
        include_bytes!("../fonts/Inter-Regular.ttf"),
        include_bytes!("../fonts/Inter-Medium.ttf"),
        include_bytes!("../fonts/Inter-SemiBold.ttf"),
        include_bytes!("../fonts/Inter-Bold.ttf"),
        include_bytes!("../fonts/Inter-Italic.ttf"),
    ];
    /// Inter's licence, which goes wherever its files do.
    pub const UI_LICENCE: &str = include_str!("../fonts/Inter-OFL.txt");
    /// Inter for the web, in `fonts/` beside the stylesheet: file, weight, style and its TTF.
    pub const UI_WEB: [(&str, u16, &str, &[u8]); 5] = [
        ("Inter-Regular.woff2", 400, "normal", UI_STATIC[0]),
        ("Inter-Medium.woff2", 500, "normal", UI_STATIC[1]),
        ("Inter-SemiBold.woff2", 600, "normal", UI_STATIC[2]),
        ("Inter-Bold.woff2", 700, "normal", UI_STATIC[3]),
        ("Inter-Italic.woff2", 400, "italic", UI_STATIC[4]),
    ];

    /// What a page in English or another Western European language draws, as a CSS
    /// `unicode-range`: Google Fonts' latin set, with the arrows, box drawing, shapes, ✓ ✗ and
    /// ❮ ❯ zirc's pages and chat draw too. A web font is served as two files,
    /// `<name>-latin.woff2` with these and `<name>-rest.woff2` with the rest of what it has
    /// ([`rest`]), and a browser fetches the rest only for a page that draws one of them:
    /// Inter's latin is 32 KB of its 111. `bin/web_fonts.py` cuts the files, reading the
    /// ranges from the stylesheet.
    pub const LATIN: &str = "U+0000-00FF, U+0131, U+0152-0153, U+02BB-02BC, U+02C6, U+02DA, \
                             U+02DC, U+0304, U+0308, U+0329, U+2000-206F, U+20AC, U+2122, \
                             U+2190-21FF, U+2212, U+2215, U+2500-25FF, U+2713, U+2717, \
                             U+276E-276F, U+FEFF, U+FFFD";

    /// The characters `ttf` has that [`LATIN`] leaves out, as a `unicode-range`. Only what
    /// it has: a range reaching past it, over emoji say, would have a browser fetch the file
    /// for a 👍 it can't draw.
    ///
    /// # Panics
    ///
    /// If `ttf` isn't a font with a Unicode cmap, or [`LATIN`] isn't hex: neither is so, which
    /// the stylesheet's test reads.
    #[must_use]
    pub fn rest(ttf: &[u8]) -> String {
        let latin = latin();
        let face = ttf_parser::Face::parse(ttf, 0).expect("a font");
        let mut has = std::collections::BTreeSet::new();
        for table in face.tables().cmap.expect("a cmap").subtables {
            if table.is_unicode() {
                table.codepoints(|c| {
                    if !latin.iter().any(|&(start, end)| (start..=end).contains(&c)) {
                        has.insert(c);
                    }
                });
            }
        }
        let mut ranges: Vec<(u32, u32)> = Vec::new();
        for c in has {
            match ranges.last_mut() {
                Some((_, end)) if *end + 1 == c => *end = c,
                _ => ranges.push((c, c)),
            }
        }
        ranges
            .into_iter()
            .map(|(start, end)| {
                if start == end {
                    format!("U+{start:04X}")
                } else {
                    format!("U+{start:04X}-{end:04X}")
                }
            })
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// [`LATIN`]'s ranges, first and last code point.
    fn latin() -> Vec<(u32, u32)> {
        LATIN
            .split(',')
            .map(|range| {
                let range = range.trim().trim_start_matches("U+");
                let (start, end) = range.split_once('-').unwrap_or((range, range));
                let hex = |digits| u32::from_str_radix(digits, 16).expect("LATIN is hex");
                (hex(start), hex(end))
            })
            .collect()
    }
}

/// zirc's macOS app icons on Apple's template, a rounded square filled with the stone's
/// facets and the braid in the middle, with its shadow, 16 to 1024 px at 1x and 2x: `art/icons/zirc-<variant>.icns`; and the iOS app
/// icon; as bytes, for an app bundle built from the crate.
pub mod icons {
    /// On the dark ground: the default.
    pub const DARK_ICNS: &[u8] = include_bytes!("../art/icons/zirc-dark.icns");
    /// On the light ground.
    pub const LIGHT_ICNS: &[u8] = include_bytes!("../art/icons/zirc-light.icns");
    /// In greys, for macOS's tinted icon style.
    pub const TINTED_ICNS: &[u8] = include_bytes!("../art/icons/zirc-tinted.icns");
    /// The iOS app icon, 1024 px: the stone to every edge with the braid in the middle, as
    /// iOS rounds it itself, and without an alpha channel, as App Store Connect asks:
    /// `art/icons/ios/zirc-1024.png`.
    pub const IOS: &[u8] = include_bytes!("../art/icons/ios/zirc-1024.png");
}

/// The agent avatars, the pictures an agent session picks from by id (zirc-proto's
/// `AGENT_AVATARS`), as SVGs in white on nothing, for apps to draw on the session's colour:
/// `art/avatars/<id>.svg`, made by zirc-theme-art.
pub mod avatars {
    /// Each picture by its id, in the gallery's order. The Android app names them again, as
    /// a genrule must (`AGENT_AVATARS` in oss/apps/android/BUILD.bazel), and checks they
    /// match: change both together.
    pub const SVGS: [(&str, &[u8]); 24] = [
        ("cat", include_bytes!("../art/avatars/cat.svg")),
        ("dog", include_bytes!("../art/avatars/dog.svg")),
        ("bird", include_bytes!("../art/avatars/bird.svg")),
        ("fish", include_bytes!("../art/avatars/fish.svg")),
        ("rabbit", include_bytes!("../art/avatars/rabbit.svg")),
        ("squirrel", include_bytes!("../art/avatars/squirrel.svg")),
        ("turtle", include_bytes!("../art/avatars/turtle.svg")),
        ("snail", include_bytes!("../art/avatars/snail.svg")),
        ("bug", include_bytes!("../art/avatars/bug.svg")),
        ("ghost", include_bytes!("../art/avatars/ghost.svg")),
        ("rocket", include_bytes!("../art/avatars/rocket.svg")),
        ("robot", include_bytes!("../art/avatars/robot.svg")),
        ("anchor", include_bytes!("../art/avatars/anchor.svg")),
        ("compass", include_bytes!("../art/avatars/compass.svg")),
        ("telescope", include_bytes!("../art/avatars/telescope.svg")),
        ("atom", include_bytes!("../art/avatars/atom.svg")),
        ("gem", include_bytes!("../art/avatars/gem.svg")),
        ("coffee", include_bytes!("../art/avatars/coffee.svg")),
        ("leaf", include_bytes!("../art/avatars/leaf.svg")),
        ("flame", include_bytes!("../art/avatars/flame.svg")),
        ("bolt", include_bytes!("../art/avatars/bolt.svg")),
        ("moon", include_bytes!("../art/avatars/moon.svg")),
        ("star", include_bytes!("../art/avatars/star.svg")),
        ("mountain", include_bytes!("../art/avatars/mountain.svg")),
    ];

    /// The picture with id `id`, as an SVG: `None` for one the gallery hasn't.
    #[must_use]
    pub fn svg(id: &str) -> Option<&'static [u8]> {
        SVGS.iter()
            .find(|(known, _)| *known == id)
            .map(|(_, svg)| *svg)
    }
}

/// The Android app's logo files, for an app built from the crate.
pub mod android {
    /// Each file of the app's `res/`, by its path there, such as
    /// `mipmap-xxxhdpi/ic_launcher_foreground.png`, and its bytes: the dark launcher icon
    /// (adaptive foreground, background and monochrome layers, the adaptive-icon XML, and a
    /// legacy icon at each density), `drawable/ic_notification.xml`, and the wordmark as
    /// `drawable/zirc_wordmark.xml` and `drawable-night/zirc_wordmark.xml`, all from
    /// `art/android`.
    pub const RES: &[(&str, &[u8])] = include!("../art/android/res.rs");
}

/// How the logo is put together: a hash braided from four strands. Alone, with zirc's name
/// after it, it is the wordmark; set in a cut stone, it is the mark that app icons and
/// favicons show. Both are drawn on a box of [`Logo::BOX`], about its middle.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Logo {
    /// The name the wordmark spells after its braid: `zirc`.
    pub wordmark: String,
    /// The name's weight: bold.
    pub wordmark_weight: u16,
    /// The name's letter spacing, in ems.
    pub wordmark_tracking_em: f32,
    /// The braid as the wordmark has it.
    pub braid: Braid,
    /// The stone, and the braid set in it.
    pub stone: Stone,
    /// How much of an app icon's side the stone takes.
    pub icon_mark_side: f32,
    /// How much of an icon's side the braid takes where it stands alone, in one colour:
    /// Android's themed icon.
    pub icon_braid_side: f32,
}

impl Logo {
    /// The side of the box the braid and the stone are drawn on.
    pub const BOX: f32 = 100.0;

    /// zirc's logo.
    #[must_use]
    pub fn get() -> &'static Logo {
        &TOKENS_PARSED.logo
    }
}

/// A value for each mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ByMode<T> {
    /// On the dark ground.
    pub dark: T,
    /// On the light ground.
    pub light: T,
}

impl<T: Copy> ByMode<T> {
    /// The one for `mode`.
    #[must_use]
    pub fn of(&self, mode: Mode) -> T {
        match mode {
            Mode::Dark => self.dark,
            Mode::Light => self.light,
        }
    }
}

/// A point on the logo's box: across from the left, down from the top.
pub type Point = (f32, f32);

/// The wordmark's braid: all its strands, across and down, in the theme's `signal` blue, as
/// filled buttons are.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Braid {
    /// Its shape.
    pub strands: Strands,
    /// How high it stands beside the name, in the name's ems.
    pub height_em: f32,
    /// How far below the name's baseline it reaches, in ems: as far as brings the top of its
    /// upper strand across level with the top of the name's `z`.
    pub drop_em: f32,
    /// The space between it and where the name's first letter begins, in ems.
    pub gap_em: f32,
}

/// A braided hash's shape: two strands across and two down, leaning as a hash does, each
/// passing over one strand and under the other.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Strands {
    /// How thick a strand is.
    pub stroke: f32,
    /// How far a strand down leans to the right for each unit it rises.
    pub lean: f32,
    /// The space left either side of a strand where another passes under it.
    pub gap: f32,
    /// How far a strand across reaches either side of the middle.
    pub reach: f32,
    /// How far a strand down rises above the middle, and falls below it.
    pub rise: f32,
    /// How far apart a pair of strands is, middle to middle.
    pub apart: f32,
}

/// A length of strand, as its four corners. A strand that passes under another is two.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Piece {
    /// Whether it's of a strand down, not across.
    pub down: bool,
    /// Its corners, in order round it.
    pub points: [Point; 4],
}

impl Strands {
    /// The braid's eight pieces, those across first.
    #[must_use]
    pub fn pieces(&self) -> Vec<Piece> {
        let middle = Logo::BOX / 2.0;
        let (near, far) = (middle - self.apart / 2.0, middle + self.apart / 2.0);
        let half = self.stroke / 2.0;
        // How far right a strand down stands at height `y`, for its lean.
        let shift = |y: f32| (middle - y) * self.lean;
        let mut pieces = Vec::new();
        // The upper strand across passes under the right strand down; the lower, the left.
        for (y, under) in [(near, far), (far, near)] {
            let (top, bottom) = (y - half, y + half);
            let (left, right) = (
                middle - self.reach + shift(y),
                middle + self.reach + shift(y),
            );
            let before = |y: f32| under + shift(y) - half - self.gap;
            let after = |y: f32| under + shift(y) + half + self.gap;
            pieces.push(Piece {
                down: false,
                points: [
                    (left, top),
                    (before(top), top),
                    (before(bottom), bottom),
                    (left, bottom),
                ],
            });
            pieces.push(Piece {
                down: false,
                points: [
                    (after(top), top),
                    (right, top),
                    (right, bottom),
                    (after(bottom), bottom),
                ],
            });
        }
        // Each strand down passes under the strand across that it meets on the diagonal.
        for x in [near, far] {
            let (top, bottom) = (middle - self.rise, middle + self.rise);
            let length = |from: f32, to: f32| Piece {
                down: true,
                points: [
                    (x + shift(from) - half, from),
                    (x + shift(from) + half, from),
                    (x + shift(to) + half, to),
                    (x + shift(to) - half, to),
                ],
            };
            pieces.push(length(top, x - half - self.gap));
            pieces.push(length(x + half + self.gap, bottom));
        }
        pieces
    }

    /// The box its ink fills: left, top, right, bottom.
    #[must_use]
    pub fn bounds(&self) -> (f32, f32, f32, f32) {
        let points = self.pieces().into_iter().flat_map(|piece| piece.points);
        points.fold(
            (f32::MAX, f32::MAX, f32::MIN, f32::MIN),
            |(left, top, right, bottom), (x, y)| {
                (left.min(x), top.min(y), right.max(x), bottom.max(y))
            },
        )
    }
}

/// The stone: a square with its corners cut, in four facets that meet at its middle, lit
/// from the top left, with a braid set in it.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Stone {
    /// Its side.
    pub side: f32,
    /// How much of a side each cut corner takes.
    pub cut: f32,
    /// Its facets' colours: the top, the left, the right and the bottom.
    pub facets: ByMode<[Rgb; 4]>,
    /// The braid set in it.
    pub strands: Strands,
    /// The colour of that braid.
    pub set: Rgb,
}

impl Stone {
    /// Its facets as their corners: the top, the left, the right and the bottom.
    #[must_use]
    pub fn facets(&self) -> [[Point; 5]; 4] {
        let middle = Logo::BOX / 2.0;
        let (lo, hi) = (middle - self.side / 2.0, middle + self.side / 2.0);
        let (cut, half) = (self.cut, self.cut / 2.0);
        let centre = (middle, middle);
        // Where the lines from the middle to the square's corners cross the cuts.
        let (top_left, top_right) = ((lo + half, lo + half), (hi - half, lo + half));
        let (bottom_left, bottom_right) = ((lo + half, hi - half), (hi - half, hi - half));
        [
            [top_left, (lo + cut, lo), (hi - cut, lo), top_right, centre],
            [
                top_left,
                centre,
                bottom_left,
                (lo, hi - cut),
                (lo, lo + cut),
            ],
            [
                top_right,
                (hi, lo + cut),
                (hi, hi - cut),
                bottom_right,
                centre,
            ],
            [
                bottom_left,
                centre,
                bottom_right,
                (hi - cut, hi),
                (lo + cut, hi),
            ],
        ]
    }
}

/// Dark or light.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Light text on a dark ground.
    Dark,
    /// Dark text on a light ground.
    Light,
}

/// A colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub struct Rgb {
    /// Red.
    pub r: u8,
    /// Green.
    pub g: u8,
    /// Blue.
    pub b: u8,
}

impl Rgb {
    /// A part of a colour or an opacity, from 0 to 1, as the byte that writes it: 0 to 255,
    /// rounded, and clamped for what's outside.
    #[must_use]
    pub fn byte(unit: f64) -> u8 {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "clamped to a byte's range first; floats have no checked conversion"
        )]
        let byte = (unit.clamp(0.0, 1.0) * 255.0).round() as u8;
        byte
    }

    /// It as CSS writes it: `#0A0C0B`.
    #[must_use]
    pub fn hex(self) -> String {
        format!("#{:02X}{:02X}{:02X}", self.r, self.g, self.b)
    }

    /// Its relative luminance, as WCAG reckons it: 0 for black, 1 for white.
    #[must_use]
    pub fn luminance(self) -> f64 {
        let linear = |c: u8| {
            let c = f64::from(c) / 255.0;
            if c <= 0.039_28 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * linear(self.r) + 0.7152 * linear(self.g) + 0.0722 * linear(self.b)
    }

    /// Its contrast with `other`, as WCAG reckons it: from 1 up to 21.
    #[must_use]
    pub fn contrast(self, other: Rgb) -> f64 {
        let (a, b) = (self.luminance(), other.luminance());
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }
}

impl TryFrom<String> for Rgb {
    type Error = String;

    fn try_from(hex: String) -> Result<Self, String> {
        let digits = hex
            .strip_prefix('#')
            .filter(|d| d.len() == 6 && d.is_ascii())
            .ok_or_else(|| format!("{hex:?} isn't #RRGGBB"))?;
        let part = |at: usize| {
            u8::from_str_radix(&digits[at..at + 2], 16)
                .map_err(|_| format!("{hex:?} isn't #RRGGBB"))
        };
        Ok(Self {
            r: part(0)?,
            g: part(2)?,
            b: part(4)?,
        })
    }
}

/// The colours, by role, in one mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Theme {
    /// Which mode.
    pub mode: Mode,
    /// What everything sits on.
    pub ground: Rgb,
    /// A raised area: a sidebar, a card, an input.
    pub panel: Rgb,
    /// Names, headings, and what's being typed.
    pub text: Rgb,
    /// What people said.
    pub body: Rgb,
    /// Times, labels, and what matters less.
    pub muted: Rgb,
    /// Lines between things.
    pub rule: Rgb,
    /// Lines that need to show: a focused input, a selection.
    pub rule_strong: Rgb,
    /// zirc's one accent, used sparingly: the wordmark's `#` and `_`, primary buttons,
    /// links, mentions and the selected item.
    pub signal: Rgb,
    /// Text on [`Theme::signal`].
    pub on_signal: Rgb,
    /// Channel names.
    pub channel: Rgb,
    /// Status that's well, and only that: connected, online, ready, done.
    pub ok: Rgb,
    /// Mentions, and what needs attention.
    pub warning: Rgb,
    /// What went wrong.
    pub error: Rgb,
    /// Inline code and code blocks sit on it.
    pub code_ground: Rgb,
    /// A message that mentions you: a tinted row, with a [`Theme::warning`] bar.
    pub mention_ground: Rgb,
    /// A mention of someone else, as a pill.
    pub pill_ground: Rgb,
    /// Code: keywords.
    pub syntax_keyword: Rgb,
    /// Code: strings.
    pub syntax_string: Rgb,
    /// Code: comments.
    pub syntax_comment: Rgb,
    /// Code: numbers and constants.
    pub syntax_number: Rgb,
    /// Code: types' names.
    pub syntax_type: Rgb,
    /// Code: functions' names.
    pub syntax_function: Rgb,
    /// Code: lines a diff adds.
    pub syntax_added: Rgb,
    /// Code: lines a diff removes.
    pub syntax_removed: Rgb,
    /// The rail and tab bar: the darkest tier in dark, the greyest in light.
    pub surface_ground: Rgb,
    /// The conversation list.
    pub surface_sidebar: Rgb,
    /// The conversation itself.
    pub surface_chat: Rgb,
    /// Cards, inputs and chips, a tier up; in light, with a [`Theme::rule`] border.
    pub surface_raised: Rgb,
    /// Popovers and sheets; in light, with elevation e2 ([`Design::elevation`]).
    pub surface_overlay: Rgb,
    /// The selected row's ground, with its text in [`Theme::text`] and its `#` in [`Theme::signal`].
    pub selected: Rgb,
    /// A count of mentions, or NEW.
    pub badge: Rgb,
    /// Text on [`Theme::badge`].
    pub on_badge: Rgb,
    /// An @mention in text, as a chip.
    pub mention_chip_ground: Rgb,
    /// Text on [`Theme::mention_chip_ground`].
    pub mention_chip_text: Rgb,
    /// The edge of a row that mentions you, on [`Theme::mention_ground`].
    pub mention_edge: Rgb,
    /// Someone's dot when they're here.
    pub presence_here: Rgb,
    /// Someone's dot when they're away.
    pub presence_away: Rgb,
    /// Someone's dot when they're busy. Gone is a ring in [`Theme::muted`].
    pub presence_busy: Rgb,
    /// A reaction you made: its ground.
    pub reaction_mine_ground: Rgb,
    /// A reaction you made: its border.
    pub reaction_mine_border: Rgb,
    /// A reaction you made: its count.
    pub reaction_mine_text: Rgb,
    /// A tonal button's ground ([`Design::button`]).
    pub tonal_ground: Rgb,
    /// Text on [`Theme::tonal_ground`].
    pub tonal_text: Rgb,
}

impl Theme {
    /// The dark theme.
    #[must_use]
    pub fn dark() -> &'static Theme {
        &THEMES.0
    }

    /// The light theme.
    #[must_use]
    pub fn light() -> &'static Theme {
        &THEMES.1
    }

    /// The theme for `mode`.
    #[must_use]
    pub fn of(mode: Mode) -> &'static Theme {
        match mode {
            Mode::Dark => Self::dark(),
            Mode::Light => Self::light(),
        }
    }

    /// The colour of the role called `name` in `tokens.json`, for what names its colour by
    /// role, as zirc-chat's icons and code tokens do.
    #[must_use]
    pub fn role(&self, name: &str) -> Option<Rgb> {
        self.roles()
            .into_iter()
            .find_map(|(role, colour)| (role == name).then_some(colour))
    }

    /// Every role, by its name in `tokens.json`, and its colour.
    #[must_use]
    pub fn roles(&self) -> [(&'static str, Rgb); 43] {
        [
            ("ground", self.ground),
            ("panel", self.panel),
            ("text", self.text),
            ("body", self.body),
            ("muted", self.muted),
            ("rule", self.rule),
            ("rule-strong", self.rule_strong),
            ("signal", self.signal),
            ("on-signal", self.on_signal),
            ("channel", self.channel),
            ("ok", self.ok),
            ("warning", self.warning),
            ("error", self.error),
            ("code-ground", self.code_ground),
            ("mention-ground", self.mention_ground),
            ("pill-ground", self.pill_ground),
            ("syntax-keyword", self.syntax_keyword),
            ("syntax-string", self.syntax_string),
            ("syntax-comment", self.syntax_comment),
            ("syntax-number", self.syntax_number),
            ("syntax-type", self.syntax_type),
            ("syntax-function", self.syntax_function),
            ("syntax-added", self.syntax_added),
            ("syntax-removed", self.syntax_removed),
            ("surface-ground", self.surface_ground),
            ("surface-sidebar", self.surface_sidebar),
            ("surface-chat", self.surface_chat),
            ("surface-raised", self.surface_raised),
            ("surface-overlay", self.surface_overlay),
            ("selected", self.selected),
            ("badge", self.badge),
            ("on-badge", self.on_badge),
            ("mention-chip-ground", self.mention_chip_ground),
            ("mention-chip-text", self.mention_chip_text),
            ("mention-edge", self.mention_edge),
            ("presence-here", self.presence_here),
            ("presence-away", self.presence_away),
            ("presence-busy", self.presence_busy),
            ("reaction-mine-ground", self.reaction_mine_ground),
            ("reaction-mine-border", self.reaction_mine_border),
            ("reaction-mine-text", self.reaction_mine_text),
            ("tonal-ground", self.tonal_ground),
            ("tonal-text", self.tonal_text),
        ]
    }
}

/// Type, the same in both modes.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Type {
    /// The `mono` role, JetBrains Mono: only for code, paths, commit hashes and session
    /// ids, which line up in it.
    pub family: String,
    /// What a page falls back to, as CSS lists it, while the family loads.
    pub fallback: String,
    /// The `ui` role, for everything else: each platform's own proportional system font,
    /// as CSS names it, `system-ui`. Native apps use their system font for it.
    pub ui: String,
    /// What a page falls back to for [`Type::ui`], as CSS lists it.
    pub ui_fallback: String,
    /// How labels are written: `uppercase`. Kept while apps move to
    /// [`Design::section_label_case`] and [`Design::tag_case`].
    pub label_case: String,
    /// The letter spacing of labels, in ems.
    pub label_tracking_em: f32,
    /// How round corners are: square. Kept while apps move to [`Design::radius`].
    pub radius_px: u8,
}

impl Type {
    /// zirc's type.
    #[must_use]
    pub fn get() -> &'static Type {
        &TOKENS_PARSED.r#type
    }

    /// `text` as a label: `CHANNELS`.
    #[must_use]
    pub fn label(&self, text: &str) -> String {
        text.to_uppercase()
    }
}

/// An agent session's avatar's ground: its hue is the session's own
/// (`zirc_core::chat::nick_degrees`), and these the same for every session, in both modes,
/// so white reads on it.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Avatar {
    /// From 0 to 1.
    pub saturation: f32,
    /// From 0 to 1.
    pub lightness: f32,
}

impl Avatar {
    /// zirc's avatar ground.
    #[must_use]
    pub fn get() -> &'static Avatar {
        &TOKENS_PARSED.avatar
    }
}

/// The design system's scales, from `tokens.json`'s `design`: state layers, the UI typeface
/// and type scale, spacing, radii, avatar sizes, presence dots, elevation, motion and
/// buttons. The same in both modes, but for elevation's colours.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Design {
    /// Tints of [`Theme::text`] over any surface, for hover and press.
    pub state: State,
    /// The `ui` role's typeface: Inter, bundled for desktop and web ([`fonts::UI_STATIC`]).
    pub ui_family: String,
    /// What a page falls back to for [`Design::ui_family`], as CSS lists it.
    pub ui_fallback: String,
    /// The type scale.
    pub text: TextScale,
    /// How section labels are written: `none`, sentence case, "Channels".
    pub section_label_case: String,
    /// How tags are written: `uppercase`, "APP".
    pub tag_case: String,
    /// Spacing, `space-1` to `space-7`, in points.
    pub space: [u16; 7],
    /// Corner radii by use, in points.
    pub radius: Radii,
    /// Avatar sizes by place, in points.
    pub avatar: AvatarSizes,
    /// A presence dot's size, as a share of its avatar's.
    pub presence_dot: f32,
    /// The ring round a presence dot, in the surface's colour, in points.
    pub presence_ring: u16,
    /// Shadows: e1 for hover bars and menus, e2 for popovers and sheets.
    pub elevation: Elevations,
    /// How long things take, and how they ease. Apps skip motion when the system asks for
    /// reduced motion.
    pub motion: Motion,
    /// Buttons: filled, tonal, outline and ghost, never bracketed.
    pub button: Buttons,
}

impl Design {
    /// zirc's design scales.
    #[must_use]
    pub fn get() -> &'static Design {
        &TOKENS_PARSED.design
    }
}

/// State layers: [`Theme::text`] at these opacities, over any surface.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct State {
    /// Under the pointer.
    pub hover: f32,
    /// Being pressed.
    pub pressed: f32,
}

/// A step of the type scale, in points.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextStyle {
    /// Its size.
    pub size: u16,
    /// Its line height.
    pub line: u16,
    /// Its weight, 400 to 700.
    pub weight: u16,
}

/// The type scale.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct TextScale {
    /// A screen's title.
    pub title: TextStyle,
    /// A conversation's header.
    pub header: TextStyle,
    /// A row's title in a phone's list.
    pub list_title: TextStyle,
    /// Messages on a phone.
    pub body_phone: TextStyle,
    /// Messages on the desktop and web.
    pub body_desktop: TextStyle,
    /// A row's second line, a preview.
    pub secondary: TextStyle,
    /// Times and small print.
    pub caption: TextStyle,
    /// A section's label, "Channels", in [`Design::section_label_case`].
    pub section_label: TextStyle,
    /// A tag, "APP", in [`Design::tag_case`].
    pub tag: TextStyle,
}

impl TextScale {
    /// Every step, by its name in `tokens.json`.
    #[must_use]
    pub fn styles(&self) -> [(&'static str, TextStyle); 9] {
        [
            ("title", self.title),
            ("header", self.header),
            ("list-title", self.list_title),
            ("body-phone", self.body_phone),
            ("body-desktop", self.body_desktop),
            ("secondary", self.secondary),
            ("caption", self.caption),
            ("section-label", self.section_label),
            ("tag", self.tag),
        ]
    }
}

/// Corner radii by use, in points.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Radii {
    /// Code blocks: square.
    pub code: u16,
    /// Tags and inline chips.
    pub tag: u16,
    /// Controls: buttons and inputs.
    pub control: u16,
    /// Cards, file frames and the hover bar.
    pub card: u16,
    /// Sheets.
    pub sheet: u16,
    /// An app's square avatar, such as CI's.
    pub app_avatar: u16,
    /// Pills, badges and avatars: fully round.
    pub full: u16,
}

impl Radii {
    /// Every radius, by its name in `tokens.json`.
    #[must_use]
    pub fn all(&self) -> [(&'static str, u16); 7] {
        [
            ("code", self.code),
            ("tag", self.tag),
            ("control", self.control),
            ("card", self.card),
            ("sheet", self.sheet),
            ("app-avatar", self.app_avatar),
            ("full", self.full),
        ]
    }
}

/// Avatar sizes by place, in points.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct AvatarSizes {
    /// In a reply's quote.
    pub quote: u16,
    /// In an Active now row.
    pub active_now: u16,
    /// In the desktop's conversation list.
    pub desktop_list: u16,
    /// In the members list.
    pub members: u16,
    /// In a phone's conversation list.
    pub phone_list: u16,
    /// Beside a message on the desktop.
    pub desktop_message: u16,
    /// Beside a message on a phone.
    pub phone_message: u16,
    /// On a profile card.
    pub profile: u16,
}

impl AvatarSizes {
    /// Every size, by its name in `tokens.json`.
    #[must_use]
    pub fn all(&self) -> [(&'static str, u16); 8] {
        [
            ("quote", self.quote),
            ("active-now", self.active_now),
            ("desktop-list", self.desktop_list),
            ("members", self.members),
            ("phone-list", self.phone_list),
            ("desktop-message", self.desktop_message),
            ("phone-message", self.phone_message),
            ("profile", self.profile),
        ]
    }
}

/// A shadow: straight down by `y`, blurred by `blur`, in points, in each mode's colour.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Shadow {
    /// How far down.
    pub y: u16,
    /// How blurred.
    pub blur: u16,
    /// On the light theme.
    pub light: ShadowColour,
    /// On the dark theme.
    pub dark: ShadowColour,
}

impl Shadow {
    /// Its colour on `mode`'s theme.
    #[must_use]
    pub fn colour(&self, mode: Mode) -> ShadowColour {
        match mode {
            Mode::Dark => self.dark,
            Mode::Light => self.light,
        }
    }
}

/// A shadow's colour.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShadowColour {
    /// Its colour.
    pub rgb: Rgb,
    /// How opaque, from 0 to 1.
    pub alpha: f32,
}

/// The shadows.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Elevations {
    /// Hover bars and menus.
    pub e1: Shadow,
    /// Popovers and sheets.
    pub e2: Shadow,
}

impl Elevations {
    /// Every shadow, by its name in `tokens.json`.
    #[must_use]
    pub fn all(&self) -> [(&'static str, Shadow); 2] {
        [("e1", self.e1), ("e2", self.e2)]
    }
}

/// How long things take, in milliseconds, and how they ease.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Motion {
    /// Hover and press.
    pub fast_ms: u16,
    /// Selection and fades.
    pub medium_ms: u16,
    /// Sheets.
    pub slow_ms: u16,
    /// How long a message a notification, link or search opened at stays highlighted,
    /// once it's in view, before its highlight fades over [`Self::reveal_fade_ms`]. With
    /// reduced motion, it holds as long and then goes at once.
    pub reveal_hold_ms: u16,
    /// How long that highlight takes to fade.
    pub reveal_fade_ms: u16,
    /// The easing, as a cubic Bézier's control points.
    pub easing: [f32; 4],
}

/// Buttons.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Buttons {
    /// Their height on each platform, in points: a phone's is its hit target.
    pub height: ButtonHeights,
    /// Their corner radius, by its name in [`Radii`].
    pub radius: String,
    /// Their label's weight.
    pub weight: u16,
    /// Each style's colours.
    pub styles: ButtonStyles,
}

impl Buttons {
    /// Their corner radius, in points.
    ///
    /// # Panics
    ///
    /// Never: a test checks the name is a radius's.
    #[must_use]
    pub fn radius_pt(&self) -> u16 {
        Design::get()
            .radius
            .all()
            .into_iter()
            .find(|(name, _)| *name == self.radius)
            .map(|(_, radius)| radius)
            .expect("the button's radius is one of tokens.json's")
    }
}

/// A button's height on each platform, in points.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ButtonHeights {
    /// The desktop app.
    pub desktop: u16,
    /// The web.
    pub web: u16,
    /// The phones.
    pub phone: u16,
}

/// Each button style.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ButtonStyles {
    /// The one thing to do: on [`Theme::signal`].
    pub filled: ButtonStyle,
    /// Something to do, quieter: on [`Theme::tonal_ground`].
    pub tonal: ButtonStyle,
    /// A choice beside another: outlined.
    pub outline: ButtonStyle,
    /// The least: words, with a hover layer.
    pub ghost: ButtonStyle,
}

impl ButtonStyles {
    /// Every style, by its name in `tokens.json`.
    #[must_use]
    pub fn all(&self) -> [(&'static str, &ButtonStyle); 4] {
        [
            ("filled", &self.filled),
            ("tonal", &self.tonal),
            ("outline", &self.outline),
            ("ghost", &self.ghost),
        ]
    }
}

/// A button style's colours, each by its role's name in [`Theme::roles`].
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ButtonStyle {
    /// Its ground; none for none.
    #[serde(default)]
    pub ground: Option<String>,
    /// Its label.
    pub text: String,
    /// Its border, a point wide; none for none.
    #[serde(default)]
    pub border: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Tokens {
    colors: Colors,
    r#type: Type,
    avatar: Avatar,
    logo: Logo,
    design: Design,
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct Colors {
    ground: Pair,
    panel: Pair,
    text: Pair,
    body: Pair,
    muted: Pair,
    rule: Pair,
    rule_strong: Pair,
    signal: Pair,
    on_signal: Pair,
    channel: Pair,
    ok: Pair,
    warning: Pair,
    error: Pair,
    code_ground: Pair,
    mention_ground: Pair,
    pill_ground: Pair,
    syntax_keyword: Pair,
    syntax_string: Pair,
    syntax_comment: Pair,
    syntax_number: Pair,
    syntax_type: Pair,
    syntax_function: Pair,
    syntax_added: Pair,
    syntax_removed: Pair,
    surface_ground: Pair,
    surface_sidebar: Pair,
    surface_chat: Pair,
    surface_raised: Pair,
    surface_overlay: Pair,
    selected: Pair,
    badge: Pair,
    on_badge: Pair,
    mention_chip_ground: Pair,
    mention_chip_text: Pair,
    mention_edge: Pair,
    presence_here: Pair,
    presence_away: Pair,
    presence_busy: Pair,
    reaction_mine_ground: Pair,
    reaction_mine_border: Pair,
    reaction_mine_text: Pair,
    tonal_ground: Pair,
    tonal_text: Pair,
}

#[derive(Deserialize, Clone, Copy)]
#[serde(deny_unknown_fields)]
struct Pair {
    dark: Rgb,
    light: Rgb,
}

static TOKENS_PARSED: LazyLock<Tokens> = LazyLock::new(|| {
    serde_json::from_str(TOKENS).unwrap_or_else(|err| panic!("tokens.json: {err}"))
});

static THEMES: LazyLock<(Theme, Theme)> = LazyLock::new(|| {
    let c = &TOKENS_PARSED.colors;
    let theme = |mode: Mode| {
        let pick = |pair: Pair| match mode {
            Mode::Dark => pair.dark,
            Mode::Light => pair.light,
        };
        Theme {
            mode,
            ground: pick(c.ground),
            panel: pick(c.panel),
            text: pick(c.text),
            body: pick(c.body),
            muted: pick(c.muted),
            rule: pick(c.rule),
            rule_strong: pick(c.rule_strong),
            signal: pick(c.signal),
            on_signal: pick(c.on_signal),
            channel: pick(c.channel),
            ok: pick(c.ok),
            warning: pick(c.warning),
            error: pick(c.error),
            code_ground: pick(c.code_ground),
            mention_ground: pick(c.mention_ground),
            pill_ground: pick(c.pill_ground),
            syntax_keyword: pick(c.syntax_keyword),
            syntax_string: pick(c.syntax_string),
            syntax_comment: pick(c.syntax_comment),
            syntax_number: pick(c.syntax_number),
            syntax_type: pick(c.syntax_type),
            syntax_function: pick(c.syntax_function),
            syntax_added: pick(c.syntax_added),
            syntax_removed: pick(c.syntax_removed),
            surface_ground: pick(c.surface_ground),
            surface_sidebar: pick(c.surface_sidebar),
            surface_chat: pick(c.surface_chat),
            surface_raised: pick(c.surface_raised),
            surface_overlay: pick(c.surface_overlay),
            selected: pick(c.selected),
            badge: pick(c.badge),
            on_badge: pick(c.on_badge),
            mention_chip_ground: pick(c.mention_chip_ground),
            mention_chip_text: pick(c.mention_chip_text),
            mention_edge: pick(c.mention_edge),
            presence_here: pick(c.presence_here),
            presence_away: pick(c.presence_away),
            presence_busy: pick(c.presence_busy),
            reaction_mine_ground: pick(c.reaction_mine_ground),
            reaction_mine_border: pick(c.reaction_mine_border),
            reaction_mine_text: pick(c.reaction_mine_text),
            tonal_ground: pick(c.tonal_ground),
            tonal_text: pick(c.tonal_text),
        }
    };
    (theme(Mode::Dark), theme(Mode::Light))
});

/// The web stylesheet: the fonts, type, and each role as a custom property such as
/// `--zirc-signal`, light unless the system or `data-theme="dark"` on the root says dark.
#[must_use]
pub fn css() -> String {
    let ty = Type::get();
    let avatar = Avatar::get();
    let mut css = String::from(
        "/* zirc's theme, generated from tokens.json by zirc-theme: don't edit it here.\n   \
         Rewrite it with: bazel run //crates/zirc-theme:css > oss/crates/zirc-theme/zirc-theme.css\n   \
         JetBrains Mono is under the SIL Open Font License: fonts/OFL.txt; Inter, too:\n   \
         fonts/Inter-OFL.txt. */\n",
    );
    let design = Design::get();
    let faces = fonts::WEB
        .iter()
        .map(|face| (ty.family.as_str(), face))
        .chain(
            fonts::UI_WEB
                .iter()
                .map(|face| (design.ui_family.as_str(), face)),
        );
    for (family, (file, weight, style, ttf)) in faces {
        let name = file.trim_end_matches(".woff2");
        let rest = fonts::rest(ttf);
        for (part, range) in [("latin", fonts::LATIN), ("rest", rest.as_str())] {
            let _ = write!(
                css,
                "\n@font-face {{\n  font-family: \"{family}\";\n  font-style: {style};\n  \
                 font-weight: {weight};\n  font-display: swap;\n  \
                 src: url(\"fonts/{name}-{part}.woff2\") format(\"woff2\");\n  \
                 unicode-range: {range};\n}}\n",
            );
        }
    }
    let roles = |theme: &Theme, indent: &str| {
        let mut out = theme
            .roles()
            .iter()
            .fold(String::new(), |mut out, (role, rgb)| {
                let _ = writeln!(out, "{indent}--zirc-{role}: {};", rgb.hex());
                out
            });
        for (name, shadow) in design.elevation.all() {
            let colour = shadow.colour(theme.mode);
            let _ = writeln!(
                out,
                "{indent}--zirc-elevation-{name}: 0 {}px {}px rgba({}, {}, {}, {});",
                shadow.y, shadow.blur, colour.rgb.r, colour.rgb.g, colour.rgb.b, colour.alpha,
            );
        }
        out
    };
    let _ = write!(
        css,
        "\n:root {{\n  color-scheme: light dark;\n  \
         --zirc-font: \"{}\", {};\n  --zirc-font-mono: var(--zirc-font);\n  \
         --zirc-font-ui: {}, {};\n  --zirc-label-tracking: {}em;\n  --zirc-radius: {}px;\n  \
         --zirc-avatar-saturation: {}%;\n  --zirc-avatar-lightness: {}%;\n{}{}}}\n",
        ty.family,
        ty.fallback,
        ty.ui,
        ty.ui_fallback,
        ty.label_tracking_em,
        ty.radius_px,
        (avatar.saturation * 100.).round(),
        (avatar.lightness * 100.).round(),
        scales(design),
        roles(Theme::light(), "  "),
    );
    let _ = write!(
        css,
        "\n@media (prefers-color-scheme: dark) {{\n  :root:not([data-theme=\"light\"]) {{\n    \
         color-scheme: dark;\n{}  }}\n}}\n",
        roles(Theme::dark(), "    "),
    );
    let _ = write!(
        css,
        "\n:root[data-theme=\"dark\"] {{\n  color-scheme: dark;\n{}}}\n\n\
         :root[data-theme=\"light\"] {{\n  color-scheme: light;\n}}\n",
        roles(Theme::dark(), "  "),
    );
    let _ = write!(
        css,
        "\n.zirc-label {{\n  text-transform: {};\n  letter-spacing: var(--zirc-label-tracking);\n}}\n",
        ty.label_case
    );
    let _ = write!(
        css,
        "\n.zirc-section-label {{\n  text-transform: {};\n}}\n\n.zirc-tag {{\n  text-transform: {};\n}}\n",
        design.section_label_case, design.tag_case,
    );
    let _ = write!(
        css,
        "\n@media (prefers-reduced-motion: reduce) {{\n  :root {{\n    \
         --zirc-motion-fast: 0ms;\n    --zirc-motion-medium: 0ms;\n    --zirc-motion-slow: 0ms;\n  }}\n}}\n",
    );
    css
}

/// The design scales as the stylesheet's custom properties, in points as pixels:
/// `--zirc-text-caption-size`, `--zirc-space-3`, `--zirc-radius-card`, `--zirc-avatar-profile`,
/// `--zirc-motion-fast`, `--zirc-button-filled-ground` and the rest.
fn scales(design: &Design) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "  --zirc-font-sans: \"{}\", {};",
        design.ui_family, design.ui_fallback
    );
    let _ = writeln!(out, "  --zirc-state-hover: {};", design.state.hover);
    let _ = writeln!(out, "  --zirc-state-pressed: {};", design.state.pressed);
    for (name, style) in design.text.styles() {
        let _ = writeln!(out, "  --zirc-text-{name}-size: {}px;", style.size);
        let _ = writeln!(out, "  --zirc-text-{name}-line: {}px;", style.line);
        let _ = writeln!(out, "  --zirc-text-{name}-weight: {};", style.weight);
    }
    for (step, space) in design.space.iter().enumerate() {
        let _ = writeln!(out, "  --zirc-space-{}: {space}px;", step + 1);
    }
    for (name, radius) in design.radius.all() {
        let _ = writeln!(out, "  --zirc-radius-{name}: {radius}px;");
    }
    for (name, size) in design.avatar.all() {
        let _ = writeln!(out, "  --zirc-avatar-{name}: {size}px;");
    }
    let _ = writeln!(out, "  --zirc-presence-dot: {};", design.presence_dot);
    let _ = writeln!(out, "  --zirc-presence-ring: {}px;", design.presence_ring);
    let motion = design.motion;
    let _ = writeln!(out, "  --zirc-motion-fast: {}ms;", motion.fast_ms);
    let _ = writeln!(out, "  --zirc-motion-medium: {}ms;", motion.medium_ms);
    let _ = writeln!(out, "  --zirc-motion-slow: {}ms;", motion.slow_ms);
    let [x1, y1, x2, y2] = motion.easing;
    let _ = writeln!(
        out,
        "  --zirc-easing: cubic-bezier({x1}, {y1}, {x2}, {y2});"
    );
    let button = &design.button;
    let _ = writeln!(out, "  --zirc-button-height: {}px;", button.height.web);
    let _ = writeln!(out, "  --zirc-button-radius: {}px;", button.radius_pt());
    let _ = writeln!(out, "  --zirc-button-weight: {};", button.weight);
    let role = |role: Option<&String>| {
        role.map_or_else(
            || "transparent".to_owned(),
            |role| format!("var(--zirc-{role})"),
        )
    };
    for (name, style) in button.styles.all() {
        let _ = writeln!(
            out,
            "  --zirc-button-{name}-ground: {};",
            role(style.ground.as_ref())
        );
        let _ = writeln!(
            out,
            "  --zirc-button-{name}-text: {};",
            role(Some(&style.text))
        );
        let _ = writeln!(
            out,
            "  --zirc-button-{name}-border: {};",
            role(style.border.as_ref())
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The roles that are read as text, and what they're read on.
    const TEXT: [&str; 8] = [
        "text", "body", "muted", "signal", "channel", "ok", "warning", "error",
    ];

    fn role(theme: &Theme, name: &str) -> Rgb {
        theme
            .roles()
            .into_iter()
            .find(|(n, _)| *n == name)
            .unwrap()
            .1
    }

    #[test]
    fn reads_the_tokens() {
        assert_eq!(Theme::dark().signal.hex(), "#7AA7FF");
        assert_eq!(Theme::light().ok.hex(), "#128246");
        assert_eq!(Theme::light().on_signal.hex(), "#FFFFFF");
        assert_eq!(Theme::of(Mode::Light).mode, Mode::Light);
        let ty = Type::get();
        assert_eq!(ty.family, "JetBrains Mono");
        assert_eq!(ty.label("channels"), "CHANNELS");
        assert!(Rgb::try_from("#12345".to_owned()).is_err());
    }

    #[test]
    fn every_text_role_reads_on_its_ground_and_panel() {
        let mut failures = Vec::new();
        for theme in [Theme::dark(), Theme::light()] {
            let pairs = TEXT
                .iter()
                .flat_map(|text| [(*text, "ground"), (*text, "panel")])
                .chain([("on-signal", "signal")]);
            for (fg, bg) in pairs {
                let ratio = role(theme, fg).contrast(role(theme, bg));
                if ratio < 4.5 {
                    failures.push(format!("{:?} {fg} on {bg}: {ratio:.2}", theme.mode));
                }
            }
        }
        assert!(failures.is_empty(), "below 4.5:1: {failures:?}");
    }

    #[test]
    fn code_and_mentions_read_on_their_grounds() {
        let mut failures = Vec::new();
        for theme in [Theme::dark(), Theme::light()] {
            let pairs = [
                "syntax-keyword",
                "syntax-string",
                "syntax-comment",
                "syntax-number",
                "syntax-type",
                "syntax-function",
                "syntax-added",
                "syntax-removed",
                "body",
                "text",
            ]
            .map(|fg| (fg, "code-ground"))
            .into_iter()
            .chain([
                ("body", "mention-ground"),
                ("text", "pill-ground"),
                ("signal", "pill-ground"),
            ]);
            for (fg, bg) in pairs {
                let ratio = role(theme, fg).contrast(role(theme, bg));
                if ratio < 4.5 {
                    failures.push(format!("{:?} {fg} on {bg}: {ratio:.2}", theme.mode));
                }
            }
        }
        assert!(failures.is_empty(), "below 4.5:1: {failures:?}");
    }

    #[test]
    fn the_stylesheet_is_up_to_date() {
        assert!(
            CSS == css(),
            "zirc-theme.css is out of date with tokens.json; rewrite it with:\n  \
             bazel run //crates/zirc-theme:css > oss/crates/zirc-theme/zirc-theme.css"
        );
        for (file, ..) in fonts::WEB.iter().chain(&fonts::UI_WEB) {
            let name = file.trim_end_matches(".woff2");
            assert!(CSS.contains(&format!("fonts/{name}-latin.woff2")));
            assert!(CSS.contains(&format!("fonts/{name}-rest.woff2")));
        }
        // Nothing from anyone else's servers.
        assert!(!CSS.contains("http"));
    }

    #[test]
    fn a_fonts_rest_is_what_it_has_beyond_latin() {
        let rest = fonts::rest(fonts::UI_STATIC[0]);
        // Cyrillic, Greek and Vietnamese, and never Latin again or an emoji it can't draw.
        for has in ["U+0410", "U+0391", "U+1EBF"] {
            assert!(
                rest.split(", ").any(|range| covers(range, has)),
                "{has} in {rest}"
            );
        }
        for not in ["U+0041", "U+2713", "U+276F", "U+1F44D"] {
            assert!(
                !rest.split(", ").any(|range| covers(range, not)),
                "{not} in {rest}"
            );
        }
    }

    fn covers(range: &str, point: &str) -> bool {
        let hex = |s: &str| u32::from_str_radix(s.trim_start_matches("U+"), 16).unwrap();
        let (start, end) = range.split_once('-').unwrap_or((range, range));
        (hex(start)..=hex(end.trim_start_matches("U+"))).contains(&hex(point))
    }

    #[test]
    fn reads_the_design_scales() {
        let design = Design::get();
        assert_eq!(design.ui_family, "Inter");
        assert_eq!(
            design.text.body_desktop,
            TextStyle {
                size: 14,
                line: 21,
                weight: 400
            }
        );
        assert_eq!(design.section_label_case, "none");
        assert_eq!(design.space, [4, 8, 12, 16, 20, 24, 32]);
        assert_eq!(design.radius.card, 10);
        assert_eq!(design.button.radius_pt(), 6);
        assert_eq!(design.avatar.profile, 80);
        assert!((design.elevation.e2.colour(Mode::Dark).alpha - 0.5).abs() < f32::EPSILON);
        assert_eq!(design.motion.fast_ms, 120);
        assert_eq!(design.motion.reveal_hold_ms, 3000);
    }

    #[test]
    fn buttons_name_colours_and_a_radius_that_exist() {
        let roles: Vec<&str> = Theme::dark()
            .roles()
            .iter()
            .map(|(name, _)| *name)
            .collect();
        for (name, style) in Design::get().button.styles.all() {
            for role in [
                style.ground.as_ref(),
                Some(&style.text),
                style.border.as_ref(),
            ]
            .into_iter()
            .flatten()
            {
                assert!(
                    roles.contains(&role.as_str()),
                    "{name} button: no role {role}"
                );
            }
        }
        let radii: Vec<&str> = Design::get().radius.all().iter().map(|(n, _)| *n).collect();
        assert!(radii.contains(&Design::get().button.radius.as_str()));
    }

    #[test]
    fn the_new_surfaces_and_accents_read() {
        let mut failures = Vec::new();
        for theme in [Theme::dark(), Theme::light()] {
            let surfaces = [
                "surface-ground",
                "surface-sidebar",
                "surface-chat",
                "surface-raised",
                "surface-overlay",
            ];
            let pairs = ["text", "body", "muted", "signal"]
                .into_iter()
                .flat_map(|fg| surfaces.map(|bg| (fg, bg)))
                .chain([
                    ("text", "selected"),
                    ("signal", "selected"),
                    ("mention-chip-text", "mention-chip-ground"),
                    ("reaction-mine-text", "reaction-mine-ground"),
                    ("tonal-text", "tonal-ground"),
                    ("on-badge", "badge"),
                ]);
            for (fg, bg) in pairs {
                let ratio = role(theme, fg).contrast(role(theme, bg));
                if ratio < 4.5 {
                    failures.push(format!("{:?} {fg} on {bg}: {ratio:.2}", theme.mode));
                }
            }
        }
        assert!(failures.is_empty(), "below 4.5:1: {failures:?}");
    }

    #[test]
    fn presence_dots_show_on_the_lists() {
        // A dot is a graphic, so 3:1 against what it sits on, as WCAG asks of one.
        let mut failures = Vec::new();
        for theme in [Theme::dark(), Theme::light()] {
            for dot in ["presence-here", "presence-away", "presence-busy"] {
                for bg in ["surface-sidebar", "surface-chat"] {
                    let ratio = role(theme, dot).contrast(role(theme, bg));
                    if ratio < 3.0 {
                        failures.push(format!("{:?} {dot} on {bg}: {ratio:.2}", theme.mode));
                    }
                }
            }
        }
        assert!(failures.is_empty(), "below 3:1: {failures:?}");
    }

    #[test]
    fn reads_the_logo() {
        let logo = Logo::get();
        assert_eq!(logo.wordmark, "zirc");
        assert!((logo.icon_mark_side - 0.7).abs() < f32::EPSILON);
        assert_eq!(logo.stone.set.hex(), "#FFFFFF");
    }

    /// Beside the name, the wordmark's upper strand across tops out where the name's `z`
    /// does: JetBrains Mono's x-height, 550 of its 1000 units.
    #[test]
    fn stands_the_braid_level_with_the_names_z() {
        let braid = &Logo::get().braid;
        let (_, top, _, bottom) = braid.strands.bounds();
        let strand_top = Logo::BOX / 2.0 - braid.strands.apart / 2.0 - braid.strands.stroke / 2.0;
        let below_top = (strand_top - top) / (bottom - top) * braid.height_em;
        let above_baseline = braid.height_em - braid.drop_em - below_top;
        assert!((above_baseline - 0.55).abs() < 0.002, "{above_baseline}");
    }

    /// Both braids are four strands in eight pieces that leave each other room, inside the
    /// box, and the stone's braid is inside the stone.
    #[test]
    fn braids_a_hash_that_fits() {
        let logo = Logo::get();
        for strands in [logo.braid.strands, logo.stone.strands] {
            let pieces = strands.pieces();
            assert_eq!(pieces.iter().filter(|piece| piece.down).count(), 4);
            assert_eq!(pieces.iter().filter(|piece| !piece.down).count(), 4);
            // An upper piece across ends a gap short of the strand that passes over it.
            let across = pieces[0].points;
            let over = pieces[1].points;
            assert!(over[0].0 - across[1].0 > strands.stroke + strands.gap);
            let (left, top, right, bottom) = strands.bounds();
            assert!(left >= 0.0 && top >= 0.0 && right <= Logo::BOX && bottom <= Logo::BOX);
        }
        let stone = &logo.stone;
        let (left, top, right, bottom) = stone.strands.bounds();
        let edge = (Logo::BOX - stone.side) / 2.0 + stone.cut / 2.0;
        assert!(left > edge && top > edge);
        assert!(right < Logo::BOX - edge && bottom < Logo::BOX - edge);
        // The facets tile the stone: together, the square less its four cut corners.
        let area: f32 = stone
            .facets()
            .iter()
            .map(|facet| {
                let turned = facet.iter().cycle().skip(1);
                let twice: f32 = facet
                    .iter()
                    .zip(turned)
                    .map(|(a, b)| a.0 * b.1 - b.0 * a.1)
                    .sum();
                twice.abs() / 2.0
            })
            .sum();
        let whole = stone.side * stone.side - 2.0 * stone.cut * stone.cut;
        assert!((area - whole).abs() < 0.01, "{area} of {whole}");
    }

    #[test]
    fn holds_androids_res_files() {
        let file = |path: &str| {
            android::RES
                .iter()
                .find(|(p, _)| *p == path)
                .unwrap_or_else(|| panic!("no {path}"))
                .1
        };
        for density in ["mdpi", "hdpi", "xhdpi", "xxhdpi", "xxxhdpi"] {
            for layer in ["foreground", "background", "monochrome"] {
                let png = file(&format!("mipmap-{density}/ic_launcher_{layer}.png"));
                assert_eq!(&png[1..4], b"PNG");
            }
        }
        let launcher = String::from_utf8_lossy(file("mipmap-anydpi-v26/ic_launcher.xml"));
        assert!(launcher.contains("<monochrome"));
        for vector in [
            "drawable/ic_notification.xml",
            "drawable/zirc_wordmark.xml",
            "drawable-night/zirc_wordmark.xml",
        ] {
            assert!(String::from_utf8_lossy(file(vector)).contains("<vector"));
        }
        // The light wordmark by day, the dark by night.
        let day = String::from_utf8_lossy(file("drawable/zirc_wordmark.xml"));
        assert!(day.contains(&Theme::light().text.hex()));
        let night = String::from_utf8_lossy(file("drawable-night/zirc_wordmark.xml"));
        assert!(night.contains(&Theme::dark().text.hex()));
    }

    #[test]
    fn ships_the_fonts() {
        // TrueType files start with version 1.0.
        assert_eq!(&fonts::MONO[..4], &[0, 1, 0, 0]);
        assert_eq!(&fonts::MONO_ITALIC[..4], &[0, 1, 0, 0]);
        assert!(fonts::LICENCE.contains("SIL Open Font License"));
        for file in fonts::UI_STATIC {
            assert_eq!(&file[..4], &[0, 1, 0, 0]);
        }
        assert!(fonts::UI_LICENCE.contains("SIL Open Font License"));
        assert!(fonts::UI_LICENCE.contains("Inter"));
    }
}
