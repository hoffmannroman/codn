//! The one built-in theme.
//!
//! Its colors are said in OKLCH and reckoned, not picked: these, and
//! every color to come. `scripts/colors.py` reckons them and says how,
//! and the tests below hold each of them to its place.

use crate::core::colors::Kind;
use crate::render::{Color, mix, rgb};
use crate::term::{DIMMED, PALETTE};

// The grays: of one hue and of nearly no chroma, each said by how light
// it is, from the ground of a pane to its text.
pub const BG: Color = rgb(0x0d0f13);
pub const SIDEBAR: Color = rgb(0x14161b);
pub const HOVER: Color = rgb(0x1d1f25);
pub const LINE: Color = rgb(0x23262d);
pub const ACTIVE: Color = rgb(0x282b32);
/// As light as it has to be to be read on the header of a pane too.
pub const FAINT: Color = rgb(0x676c75);
pub const DIM: Color = rgb(0x8b8f98);
/// White of the terminal, which is less than its text.
const WHITE: Color = rgb(0xc8cad0);
pub const TEXT: Color = rgb(0xd7d9dd);

// The six hues at the step of the text: equally light and equally
// colorful, so that none is louder than another.
const RED: Color = rgb(0xe69399);
const AMBER: Color = rgb(0xd3a563);
const GREEN: Color = rgb(0x92bc7c);
const CYAN: Color = rgb(0x53c2c1);
const BLUE: Color = rgb(0x82b1ed);
const MAGENTA: Color = rgb(0xc69bda);
// And at the step of the bright half of the terminal's table.
const RED_BRIGHT: Color = rgb(0xf3afb3);
const AMBER_BRIGHT: Color = rgb(0xe3bd8a);
const GREEN_BRIGHT: Color = rgb(0xaed09c);
const CYAN_BRIGHT: Color = rgb(0x83d5d4);
const BLUE_BRIGHT: Color = rgb(0xa1c7f8);
const MAGENTA_BRIGHT: Color = rgb(0xd8b5e8);

/// The hue the six are counted from, that of #00d4d4, at the step of
/// the others.
pub const ACCENT: Color = CYAN;
/// A link in a file that is shown as it is meant.
pub const LINK: Color = BLUE;
pub const WORKING: Color = AMBER;
pub const ATTENTION: Color = BLUE;
/// Red a step darker and of a chroma of its own: it warns, and it lies
/// under the cross that ends codn.
pub const DANGER: Color = rgb(0xe84d66);
/// A session whose program says it is done.
pub const DONE: Color = GREEN;
pub const ADDED: Color = GREEN;
pub const REMOVED: Color = RED;
pub const ADDED_BG: Color = rgb(0x182312);
pub const REMOVED_BG: Color = rgb(0x2e181a);
pub const SELECTION: Color = rgb(0x263c57);
/// What lies over the pane that has not the keyboard, beside one that
/// has: the ground, let through in part. The graphics card blends it
/// with whatever the pane shows, the colors of a program too, and not as
/// the eye takes it. A test holds that the six hues are one step under
/// it still.
pub const VEIL: Color = [BG[0], BG[1], BG[2], 110];
/// The ground under a picture, which shows what of it is transparent: a
/// chequer of cells this wide, of two grays. On it a drawing in black is
/// seen and one in white.
pub const GROUND: Color = rgb(0x6e737d);
const GROUND_LIGHT: Color = rgb(0x9498a1);
pub const GROUND_CELL: f32 = 8.0;
/// By how much every other cell is lighter, as the screen is given it.
pub const GROUND_LIGHTER: f32 = (GROUND_LIGHT[1] - GROUND[1]) as f32 / 255.0;

/// The seven colors of syntax, in the order of `Kind` behind the text
/// that is none of them. They have to be read on the surface of a line
/// that changed and in a selection too, so a comment is not faint. No
/// bold and no italic: the plain look is the choice.
const SYNTAX: [Color; 8] = [
    TEXT,    // none of them
    DIM,     // a comment
    GREEN,   // a string
    CYAN,    // a number, a value with a name, an escape
    MAGENTA, // a keyword
    BLUE,    // a function
    AMBER,   // a type
    RED,     // a tag, an attribute, a property, a key
];

/// The color a piece of a text has by its kind.
pub fn syntax(kind: Kind) -> Color {
    SYNTAX[kind as usize]
}

/// The sixteen colors a program names by number: black, the six hues in
/// the order a terminal has them, white, and the same again, bright.
/// Black is the gray of what is chosen, bright black the faint one, in
/// which programs write their hints, and bright white is white itself,
/// the gray of the lightness 1.
const ANSI: [Color; 16] = [
    ACTIVE,
    RED,
    GREEN,
    AMBER,
    BLUE,
    MAGENTA,
    CYAN,
    WHITE,
    FAINT,
    RED_BRIGHT,
    GREEN_BRIGHT,
    AMBER_BRIGHT,
    BLUE_BRIGHT,
    MAGENTA_BRIGHT,
    CYAN_BRIGHT,
    rgb(0xffffff),
];

/// The terminal color table: 16 base colors, the 6x6x6 cube, 24 grays, then
/// the named colors (256 foreground, 257 background, 258 cursor, 259 to 266
/// dim variants of the base colors, 267 bright and 268 dim foreground).
/// The cube and the 24 grays are what every terminal has there: a program
/// that asks for one of them by its number counts on that color.
pub fn palette() -> [Color; PALETTE] {
    let mut p = [TEXT; PALETTE];
    for (i, &color) in ANSI.iter().enumerate() {
        p[i] = color;
        if i < 8 {
            p[259 + i] = mix(color, BG, DIMMED);
        }
    }
    let steps = [0u8, 95, 135, 175, 215, 255];
    for i in 0..216 {
        p[16 + i] = [steps[i / 36], steps[i / 6 % 6], steps[i % 6], 255];
    }
    for i in 0..24 {
        let v = 8 + 10 * i as u8;
        p[232 + i] = [v, v, v, 255];
    }
    p[257] = BG;
    p[268] = DIM;
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    /// How far two colors are apart in lightness, as the rule for text
    /// that can be read reckons it: from 1, the same, to 21.
    fn contrast(one: Color, two: Color) -> f32 {
        let light = |color: Color| {
            let part = |value: u8| {
                let value = value as f32 / 255.0;
                if value <= 0.03928 { value / 12.92 } else { ((value + 0.055) / 1.055).powf(2.4) }
            };
            0.2126 * part(color[0]) + 0.7152 * part(color[1]) + 0.0722 * part(color[2])
        };
        let (one, two) = (light(one), light(two));
        (one.max(two) + 0.05) / (one.min(two) + 0.05)
    }

    /// A color as the eye takes it (OKLCH): how light it is, from 0 to 1,
    /// how far from gray, and its hue in degrees.
    fn seen(color: Color) -> (f64, f64, f64) {
        let [light, a, b] = crate::render::oklab(color);
        (light, a.hypot(b), b.atan2(a).to_degrees().rem_euclid(360.0))
    }

    /// The least and the most of some numbers.
    fn span(of: impl Iterator<Item = f64> + Clone) -> (f64, f64) {
        (of.clone().fold(f64::MAX, f64::min), of.fold(f64::MIN, f64::max))
    }

    const SIX: [Color; 6] = [RED, AMBER, GREEN, CYAN, BLUE, MAGENTA];

    /// How far two hues are apart, the short way round.
    fn apart(one: f64, two: f64) -> f64 {
        let turn = (one - two).rem_euclid(360.0);
        turn.min(360.0 - turn)
    }

    #[test]
    fn a_color_with_a_hue_lies_on_its_step() {
        // The six hues, 60 degrees apart from the one of the accent.
        let [red, amber, green, cyan, blue, magenta] = [15.0, 75.0, 135.0, 195.0, 255.0, 315.0];
        assert!(apart(seen(rgb(0x00d4d4)).2, cyan) < 1.0);
        // A step is one lightness and one chroma for all its hues. What a
        // screen can show lies a little beside what was reckoned.
        let step = |name: &str, light: f64, chroma: f64, colors: &[(Color, f64)]| {
            for &(color, hue) in colors {
                let (l, c, h) = seen(color);
                assert!((l - light).abs() < 0.004, "{name} at {hue}: as light as {l}");
                assert!((c - chroma).abs() < 0.004, "{name} at {hue}: as colorful as {c}");
                assert!(apart(h, hue) < 2.0, "{name} at {hue}: of the hue {h}");
            }
        };
        let text =
            [(RED, red), (AMBER, amber), (GREEN, green), (CYAN, cyan), (BLUE, blue), (MAGENTA, magenta)];
        assert_eq!(text.map(|(color, _)| color), SIX);
        step("text", 0.75, 0.10, &text);
        let bright = [
            (RED_BRIGHT, red),
            (AMBER_BRIGHT, amber),
            (GREEN_BRIGHT, green),
            (CYAN_BRIGHT, cyan),
            (BLUE_BRIGHT, blue),
            (MAGENTA_BRIGHT, magenta),
        ];
        step("bright", 0.82, 0.08, &bright);
        step("alarm", 0.64, 0.19, &[(DANGER, red)]);
        step("selection", 0.35, 0.055, &[(SELECTION, blue)]);
        step("line", 0.24, 0.035, &[(ADDED_BG, green), (REMOVED_BG, red)]);
    }

    #[test]
    fn a_gray_lies_where_its_lightness_puts_it() {
        // Of the hue 265, and of a chroma that is none at black and at
        // white and 0.016 between them. So near gray one step of the
        // screen turns the hue by degrees: what is held is how far the
        // color lies from where it was reckoned, not its angle.
        for (name, color, light) in [
            ("BG", BG, 0.17),
            ("SIDEBAR", SIDEBAR, 0.20),
            ("HOVER", HOVER, 0.24),
            ("LINE", LINE, 0.27),
            ("ACTIVE", ACTIVE, 0.29),
            ("FAINT", FAINT, 0.53),
            ("GROUND", GROUND, 0.555),
            ("GROUND_LIGHT", GROUND_LIGHT, 0.68),
            ("DIM", DIM, 0.65),
            ("WHITE", WHITE, 0.84),
            ("TEXT", TEXT, 0.885),
            ("white itself", rgb(0xffffff), 1.0),
        ] {
            let chroma = 0.016 * 4.0 * light * (1.0 - light);
            let (sin, cos) = 265.0_f64.to_radians().sin_cos();
            let [l, a, b] = crate::render::oklab(color);
            assert!((l - light).abs() < 0.004, "{name}: as light as {l}");
            let off = (a - chroma * cos).hypot(b - chroma * sin);
            assert!(off < 0.003, "{name}: {off} from its place");
        }
    }

    #[test]
    fn the_chequer_under_a_picture_is_of_two_grays() {
        // The screen is given one number for all three parts of the
        // lighter cell: it lands within a step of the gray that was
        // reckoned.
        let lighter = (GROUND_LIGHTER * 255.0).round() as i32;
        for part in 0..3 {
            let drawn = GROUND[part] as i32 + lighter;
            assert!((drawn - GROUND_LIGHT[part] as i32).abs() <= 1, "{drawn} for {}", GROUND_LIGHT[part]);
        }
    }

    #[test]
    fn what_is_dimmed_or_veiled_is_a_step_still() {
        let step = |name: &str, colors: [Color; 6]| {
            let (dark, light) = span(colors.iter().map(|&color| seen(color).0));
            let (gray, colorful) = span(colors.iter().map(|&color| seen(color).1));
            assert!(light - dark < 0.006, "{name}: from {dark} to {light}");
            assert!(colorful - gray < 0.008, "{name}: from {gray} to {colorful}");
            // Its hue goes a little towards that of the ground it is
            // blended with.
            for (color, of) in colors.iter().zip(SIX) {
                assert!(apart(seen(*color).2, seen(of).2) < 5.0, "{name}: {color:?}");
            }
        };
        // What a program dims, as codn reckons it.
        step("dimmed", SIX.map(|color| mix(color, BG, DIMMED)));
        step("dimmed, by its number", [1, 3, 2, 6, 4, 5].map(|at| palette()[259 + at]));
        // The veil, as the graphics card blends it: part by part, in what
        // the screen is given.
        let part = |color: u8, ground: u8| {
            ((color as u32 * (255 - VEIL[3] as u32) + ground as u32 * VEIL[3] as u32) / 255) as u8
        };
        step(
            "veiled",
            SIX.map(|color| [part(color[0], BG[0]), part(color[1], BG[1]), part(color[2], BG[2]), 255]),
        );
    }

    #[test]
    fn a_blend_goes_the_way_the_eye_takes_it() {
        assert_eq!(mix(TEXT, BG, 0), TEXT);
        assert_eq!(mix(TEXT, BG, 255), BG);
        // Half the way is half the lightness between them.
        let half = seen(mix(TEXT, BG, 128)).0;
        assert!((half - (seen(TEXT).0 + seen(BG).0) / 2.0).abs() < 0.004, "{half}");
        // And every color comes back from being said as the eye takes it.
        for color in SIX.into_iter().chain([BG, TEXT, DANGER, rgb(0xffffff), rgb(0x000000)]) {
            assert_eq!(crate::render::from_oklab(crate::render::oklab(color)), color);
        }
    }

    #[test]
    fn the_icon_is_in_the_colors_of_the_app() {
        // The tile and the mark, as `scripts/mark.py` wrote them.
        let icon = include_str!("../../assets/icon.svg");
        for color in [SIDEBAR, TEXT] {
            let hex = format!("#{:02x}{:02x}{:02x}", color[0], color[1], color[2]);
            assert!(icon.contains(&hex), "{hex} is not in the icon");
        }
    }

    /// A color that is written elsewhere was not reckoned, and no test
    /// here holds it.
    #[test]
    fn no_color_is_written_outside_the_theme() {
        fn read(folder: &std::path::Path, found: &mut Vec<String>) {
            for entry in std::fs::read_dir(folder).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    read(&path, found);
                } else if !path.ends_with("ui/theme.rs")
                    && std::fs::read_to_string(&path).is_ok_and(|text| text.contains("rgb(0x"))
                {
                    found.push(path.display().to_string());
                }
            }
        }
        let mut found = Vec::new();
        read(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut found);
        assert!(found.is_empty(), "a color outside src/ui/theme.rs: {found:?}");
    }

    #[test]
    fn what_is_faint_can_still_be_read() {
        // Faint is for what need not be read at once, not for what cannot be:
        // on the ground, the sidebar and the header of a pane.
        for ground in [BG, SIDEBAR, HOVER] {
            assert!(contrast(FAINT, ground) >= 3.0, "{}", contrast(FAINT, ground));
        }
        for color in [WORKING, ATTENTION, DONE, DANGER, ADDED, REMOVED] {
            assert!(contrast(color, SIDEBAR) >= 4.5, "{}", contrast(color, SIDEBAR));
        }
    }

    #[test]
    fn the_colors_of_syntax_can_be_read_where_they_stand() {
        assert_eq!(syntax(Kind::Plain), TEXT);
        for (at, kind) in Kind::ALL.into_iter().enumerate() {
            let color = syntax(kind);
            // No two kinds look alike.
            assert!(Kind::ALL[..at].iter().all(|other| syntax(*other) != color), "{kind:?}");
            // On the ground of a pane and on a line that changed, as text
            // has to be; in a selection, which is passing, a little less.
            assert!(contrast(color, BG) >= 4.5, "{kind:?} on the ground: {}", contrast(color, BG));
            assert!(
                contrast(color, ADDED_BG) >= 4.5,
                "{kind:?} on a changed line: {}",
                contrast(color, ADDED_BG)
            );
            assert!(
                contrast(color, SELECTION) >= 3.0,
                "{kind:?} in a selection: {}",
                contrast(color, SELECTION)
            );
        }
    }
}
