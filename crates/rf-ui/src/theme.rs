//! The look of the app as design tokens (THEME-01, THEME-03): its colors, fonts, sizes, icon
//! style and whether shapes also mark what the colors tell, read from a TOML file. The three
//! built-in themes, 밝게, 어둡게 and 고대비, are the files `crates/rf-ui/themes/*.toml`, built into
//! the program: no theme file is read from the disk. `docs/ux/themes.md` lists the tokens and
//! what each means.
//!
//! [`parse`] takes a file only whole: every key must be there, each with a value of its kind
//! (a color `#RRGGBB` or `#RRGGBBAA`, a size or a strength within its bounds, a font name, one
//! of a few words, true or false), and a key it does not know is refused too, so that a typo shows
//! instead of leaving a token at some default. The error names the key. A file that is refused
//! changes nothing: the theme shown stays (THEME-05's rule, for the import that comes later).
//!
//! [`builtin`] picks the theme of a [`ThemeChoice`]: 시스템 설정 따르기 takes 고대비 while the
//! system asks for high contrast, else 어둡게 or 밝게 as the system's own colors are.

use std::ops::RangeInclusive;
use std::sync::OnceLock;

use crate::settings::{Density, FontScale, ThemeChoice};

/// A color: red, green, blue and alpha (255 is opaque).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Rgba {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
    pub alpha: u8,
}

impl Rgba {
    /// An opaque color.
    pub const fn rgb(red: u8, green: u8, blue: u8) -> Self {
        Self {
            red,
            green,
            blue,
            alpha: 255,
        }
    }
}

/// How the icons are drawn (`icon-style`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum IconStyle {
    /// Thin lines (`line`): the mockup's 1.8 px.
    #[default]
    Line,
    /// Lines a third heavier (`bold`), for the high contrast theme.
    Bold,
}

impl IconStyle {
    /// How much heavier than [`IconStyle::Line`] the icons' lines are drawn.
    pub fn weight(self) -> f32 {
        match self {
            Self::Line => 1.0,
            Self::Bold => 4.0 / 3.0,
        }
    }
}

/// The colors of a theme (`[color]`), named as the `Theme` global of the screens names them.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Colors {
    /// The window behind the panes.
    pub bg: Rgba,
    /// The panes, the sidebar, dialogs and menus.
    pub panel: Rgba,
    /// What the pointer is over, and fields.
    pub raised: Rgba,
    pub text: Rgba,
    /// Second text: sizes, dates, hints.
    pub muted: Rgba,
    /// Hints and placeholders, and what is dimmed in a tree.
    pub faint: Rgba,
    /// The text and icons of a control that cannot be used now (a button, a choice, a target).
    pub disabled: Rgba,
    /// Lines between parts, and frames.
    pub border: Rgba,
    /// Under a pressed button, an open menu's button, a hovered tab or segment.
    pub pressed: Rgba,
    /// The folders' icons.
    pub folder: Rgba,
    /// The icon of what warns (an overwrite); the words beside it stay `text`.
    pub warning: Rgba,
    /// The keyboard's ring, the place of a drop, the main button.
    pub accent: Rgba,
    /// Text and icons on the accent.
    pub on_accent: Rgba,
    /// Behind a selected item, whose text stays `text`.
    pub selection: Rgba,
    /// Behind the selected text in a field.
    pub text_selection: Rgba,
    /// The selected text in a field.
    pub on_text_selection: Rgba,
    /// The notices in the window's corner, and a refused drop's hint.
    pub toast_bg: Rgba,
    pub toast_text: Rgba,
    /// A notice's action (되돌리기).
    pub toast_link: Rgba,
    /// Over a notice's button the pointer is over, and one pressed.
    pub toast_hover: Rgba,
    pub toast_pressed: Rgba,
    /// The shadow of menus, the pointer's hints and the long press's ring.
    pub shadow: Rgba,
    /// The shadow of the chosen part of a segmented choice.
    pub shadow_small: Rgba,
    /// The shadow of a notice.
    pub shadow_toast: Rgba,
    /// The shadow of a dialog.
    pub shadow_dialog: Rgba,
    /// Over the window behind a dialog or quick look.
    pub scrim: Rgba,
    /// Over the destination picker while a dialog shows on it.
    pub scrim_light: Rgba,
}

impl Colors {
    /// Every color with its key, in the order of `[color]`.
    pub fn named(&self) -> [(&'static str, Rgba); 27] {
        [
            ("bg", self.bg),
            ("panel", self.panel),
            ("raised", self.raised),
            ("text", self.text),
            ("muted", self.muted),
            ("faint", self.faint),
            ("disabled", self.disabled),
            ("border", self.border),
            ("pressed", self.pressed),
            ("folder", self.folder),
            ("warning", self.warning),
            ("accent", self.accent),
            ("on-accent", self.on_accent),
            ("selection", self.selection),
            ("text-selection", self.text_selection),
            ("on-text-selection", self.on_text_selection),
            ("toast-bg", self.toast_bg),
            ("toast-text", self.toast_text),
            ("toast-link", self.toast_link),
            ("toast-hover", self.toast_hover),
            ("toast-pressed", self.toast_pressed),
            ("shadow", self.shadow),
            ("shadow-small", self.shadow_small),
            ("shadow-toast", self.shadow_toast),
            ("shadow-dialog", self.shadow_dialog),
            ("scrim", self.scrim),
            ("scrim-light", self.scrim_light),
        ]
    }
}

/// The fonts of a theme (`[font]`): the families (empty: the system's) and the sizes of the
/// text, in logical pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct Fonts {
    /// The family of everything but the lists of items and the fixed width text.
    pub ui: String,
    /// The family of the lists of items (자세히 and the icon views).
    pub list: String,
    /// The fixed width family of a text's preview and of checksums (empty: each system's own).
    pub mono: String,
    /// Small text: columns' heads, counts, second lines.
    pub small: f32,
    /// Notes and hints beside the main text of a dialog.
    pub note: f32,
    /// The window's text.
    pub body: f32,
    /// The app's name in the toolbar.
    pub app_name: f32,
    /// The heading of a notice, of a preview and of a folder that cannot open.
    pub heading: f32,
    /// The title of a dialog.
    pub title: f32,
}

/// The spaces and shapes of a theme (`[size]`), in logical pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct Sizes {
    pub gap_xs: f32,
    pub gap_s: f32,
    pub gap_m: f32,
    pub gap_l: f32,
    /// The height of a row of the lists.
    pub row_height: f32,
    /// The corners of buttons, fields, rows and dialogs.
    pub radius: f32,
    /// The corners of the panes and the sidebar.
    pub radius_large: f32,
}

/// A theme: what a token file says.
#[derive(Clone, Debug, PartialEq)]
pub struct Tokens {
    /// The system's own widgets (scroll bars, menus, fields) are drawn dark (`scheme`).
    pub dark: bool,
    pub icon_style: IconStyle,
    /// What the colors tell is also told by a shape (`marks`): a bar beside a selected item,
    /// a badge on a cut or hidden item's icon, a dashed frame around a disabled control, a
    /// heavier frame around the chosen one of a set, a frame around a dialog (고대비).
    pub marks: bool,
    /// How strong a cut item shows (`dim-cut`, its opacity), and a hidden one (`dim-hidden`).
    pub dim_cut: f32,
    pub dim_hidden: f32,
    pub color: Colors,
    pub font: Fonts,
    pub size: Sizes,
}

/// Why a token file was refused. Every error but [`ThemeError::Syntax`] names its key, as
/// `section.key`.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ThemeError {
    /// The file is not TOML.
    #[error("the theme is not a TOML file: {0}")]
    Syntax(String),
    /// A section or a key is not there.
    #[error("the theme has no `{0}`")]
    Missing(String),
    /// A section or a key that no theme has: most likely a typo.
    #[error("the theme has `{0}`, which no theme has")]
    Unknown(String),
    /// A key has a value it cannot take.
    #[error("the theme's `{key}` is not {expected}")]
    Invalid { key: String, expected: String },
}

impl ThemeError {
    /// The section or the key the error is about, as `section.key`; none for a file that is
    /// not TOML.
    pub fn key(&self) -> Option<&str> {
        match self {
            Self::Syntax(_) => None,
            Self::Missing(key) | Self::Unknown(key) | Self::Invalid { key, .. } => Some(key),
        }
    }
}

/// The keys of `[theme]`.
const THEME_KEYS: &[&str] = &["scheme", "icon-style", "marks", "dim-cut", "dim-hidden"];
/// The keys of `[color]`.
const COLOR_KEYS: &[&str] = &[
    "bg",
    "panel",
    "raised",
    "text",
    "muted",
    "faint",
    "disabled",
    "border",
    "pressed",
    "folder",
    "warning",
    "accent",
    "on-accent",
    "selection",
    "text-selection",
    "on-text-selection",
    "toast-bg",
    "toast-text",
    "toast-link",
    "toast-hover",
    "toast-pressed",
    "shadow",
    "shadow-small",
    "shadow-toast",
    "shadow-dialog",
    "scrim",
    "scrim-light",
];
/// The keys of `[font]`.
const FONT_KEYS: &[&str] = &[
    "ui", "list", "mono", "small", "note", "body", "app-name", "heading", "title",
];
/// The keys of `[size]`.
const SIZE_KEYS: &[&str] = &[
    "gap-xs",
    "gap-s",
    "gap-m",
    "gap-l",
    "row-height",
    "radius",
    "radius-large",
];

/// The font sizes a theme may give, in logical pixels: from what still reads to what still fits.
pub const FONT_SIZES: RangeInclusive<f32> = 8.0..=72.0;
/// The gaps.
pub const GAPS: RangeInclusive<f32> = 1.0..=64.0;
/// The row heights: a row holds an icon and a line of text.
pub const ROW_HEIGHTS: RangeInclusive<f32> = 16.0..=128.0;
/// The corner radii.
pub const RADII: RangeInclusive<f32> = 1.0..=64.0;
/// The strengths of a dimmed item: it stays in sight.
pub const DIMS: RangeInclusive<f32> = 0.2..=1.0;
/// The longest font name, in characters.
pub const MAX_FONT_NAME: usize = 100;

/// The theme a token file `text` describes, or why it cannot be used.
pub fn parse(text: &str) -> Result<Tokens, ThemeError> {
    let mut file: toml::Table = text
        .parse()
        .map_err(|error: toml::de::Error| ThemeError::Syntax(error.message().to_owned()))?;
    // A section no theme has is a typo; it is told before a missing one, which it may stand for.
    let sections = ["theme", "color", "font", "size"];
    if let Some(name) = file.keys().find(|name| !sections.contains(&name.as_str())) {
        return Err(ThemeError::Unknown(name.clone()));
    }
    let mut theme = Section::take(&mut file, "theme", THEME_KEYS)?;
    let mut color = Section::take(&mut file, "color", COLOR_KEYS)?;
    let mut font = Section::take(&mut file, "font", FONT_KEYS)?;
    let mut size = Section::take(&mut file, "size", SIZE_KEYS)?;
    Ok(Tokens {
        dark: theme.word("scheme", &[("light", false), ("dark", true)])?,
        icon_style: theme.word(
            "icon-style",
            &[("line", IconStyle::Line), ("bold", IconStyle::Bold)],
        )?,
        marks: theme.flag("marks")?,
        dim_cut: theme.number("dim-cut", &DIMS)?,
        dim_hidden: theme.number("dim-hidden", &DIMS)?,
        color: Colors {
            bg: color.color("bg")?,
            panel: color.color("panel")?,
            raised: color.color("raised")?,
            text: color.color("text")?,
            muted: color.color("muted")?,
            faint: color.color("faint")?,
            disabled: color.color("disabled")?,
            border: color.color("border")?,
            pressed: color.color("pressed")?,
            folder: color.color("folder")?,
            warning: color.color("warning")?,
            accent: color.color("accent")?,
            on_accent: color.color("on-accent")?,
            selection: color.color("selection")?,
            text_selection: color.color("text-selection")?,
            on_text_selection: color.color("on-text-selection")?,
            toast_bg: color.color("toast-bg")?,
            toast_text: color.color("toast-text")?,
            toast_link: color.color("toast-link")?,
            toast_hover: color.color("toast-hover")?,
            toast_pressed: color.color("toast-pressed")?,
            shadow: color.color("shadow")?,
            shadow_small: color.color("shadow-small")?,
            shadow_toast: color.color("shadow-toast")?,
            shadow_dialog: color.color("shadow-dialog")?,
            scrim: color.color("scrim")?,
            scrim_light: color.color("scrim-light")?,
        },
        font: Fonts {
            ui: font.family("ui")?,
            list: font.family("list")?,
            mono: font.family("mono")?,
            small: font.number("small", &FONT_SIZES)?,
            note: font.number("note", &FONT_SIZES)?,
            body: font.number("body", &FONT_SIZES)?,
            app_name: font.number("app-name", &FONT_SIZES)?,
            heading: font.number("heading", &FONT_SIZES)?,
            title: font.number("title", &FONT_SIZES)?,
        },
        size: Sizes {
            gap_xs: size.number("gap-xs", &GAPS)?,
            gap_s: size.number("gap-s", &GAPS)?,
            gap_m: size.number("gap-m", &GAPS)?,
            gap_l: size.number("gap-l", &GAPS)?,
            row_height: size.number("row-height", &ROW_HEIGHTS)?,
            radius: size.number("radius", &RADII)?,
            radius_large: size.number("radius-large", &RADII)?,
        },
    })
}

/// One section of a token file, whose keys are taken one by one.
struct Section {
    name: &'static str,
    table: toml::Table,
}

impl Section {
    /// Takes the section `name` out of `file`, refusing a key in it that is not in `keys`.
    fn take(file: &mut toml::Table, name: &'static str, keys: &[&str]) -> Result<Self, ThemeError> {
        let table = match file.remove(name) {
            Some(toml::Value::Table(table)) => table,
            Some(_) => {
                return Err(ThemeError::Invalid {
                    key: name.to_owned(),
                    expected: "a section ([…])".to_owned(),
                });
            }
            None => return Err(ThemeError::Missing(name.to_owned())),
        };
        if let Some(key) = table.keys().find(|key| !keys.contains(&key.as_str())) {
            return Err(ThemeError::Unknown(format!("{name}.{key}")));
        }
        Ok(Self { name, table })
    }

    /// The value of `key`, taken out of the section.
    fn value(&mut self, key: &str) -> Result<toml::Value, ThemeError> {
        self.table
            .remove(key)
            .ok_or_else(|| ThemeError::Missing(self.full(key)))
    }

    fn full(&self, key: &str) -> String {
        format!("{}.{key}", self.name)
    }

    fn invalid(&self, key: &str, expected: impl Into<String>) -> ThemeError {
        ThemeError::Invalid {
            key: self.full(key),
            expected: expected.into(),
        }
    }

    /// A color `#RRGGBB` or `#RRGGBBAA`.
    fn color(&mut self, key: &str) -> Result<Rgba, ThemeError> {
        let value = self.value(key)?;
        value
            .as_str()
            .and_then(color_of)
            .ok_or_else(|| self.invalid(key, "a color (#RRGGBB or #RRGGBBAA)"))
    }

    /// A number within `bounds` (a size in logical pixels, or a strength): a whole number or a
    /// decimal one.
    fn number(&mut self, key: &str, bounds: &RangeInclusive<f32>) -> Result<f32, ThemeError> {
        let value = self.value(key)?;
        #[expect(
            clippy::cast_precision_loss,
            clippy::cast_possible_truncation,
            reason = "a number is checked against bounds far below where f32 loses whole pixels"
        )]
        let number = match value {
            toml::Value::Integer(number) => Some(number as f32),
            toml::Value::Float(number) => Some(number as f32),
            _ => None,
        };
        number
            .filter(|number| bounds.contains(number))
            .ok_or_else(|| {
                let (low, high) = (bounds.start(), bounds.end());
                self.invalid(key, format!("a number from {low} to {high}"))
            })
    }

    /// A font family's name, empty for the system's: at most [`MAX_FONT_NAME`] characters, no
    /// control character.
    fn family(&mut self, key: &str) -> Result<String, ThemeError> {
        let value = self.value(key)?;
        value
            .as_str()
            .filter(|name| {
                name.chars().count() <= MAX_FONT_NAME && !name.chars().any(char::is_control)
            })
            .map(str::to_owned)
            .ok_or_else(|| {
                self.invalid(
                    key,
                    format!("a font name of at most {MAX_FONT_NAME} characters, or \"\""),
                )
            })
    }

    /// `true` or `false`.
    fn flag(&mut self, key: &str) -> Result<bool, ThemeError> {
        let value = self.value(key)?;
        value
            .as_bool()
            .ok_or_else(|| self.invalid(key, "true or false"))
    }

    /// One of the `words`, as what it stands for.
    fn word<T: Copy>(&mut self, key: &str, words: &[(&str, T)]) -> Result<T, ThemeError> {
        let value = self.value(key)?;
        let found = value
            .as_str()
            .and_then(|text| words.iter().find(|(word, _)| *word == text));
        found.map(|(_, meaning)| *meaning).ok_or_else(|| {
            let names: Vec<String> = words
                .iter()
                .map(|(word, _)| format!("\"{word}\""))
                .collect();
            self.invalid(key, format!("one of {}", names.join(", ")))
        })
    }
}

/// The color `#RRGGBB` or `#RRGGBBAA` (either case) of `text`.
fn color_of(text: &str) -> Option<Rgba> {
    let digits = text.strip_prefix('#')?;
    if !(digits.len() == 6 || digits.len() == 8) || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let byte = |at: usize| u8::from_str_radix(&digits[at..at + 2], 16).ok();
    Some(Rgba {
        red: byte(0)?,
        green: byte(2)?,
        blue: byte(4)?,
        alpha: if digits.len() == 8 { byte(6)? } else { 255 },
    })
}

/// A theme built into the program.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Builtin {
    Light,
    Dark,
    HighContrast,
}

impl Builtin {
    pub const ALL: [Self; 3] = [Self::Light, Self::Dark, Self::HighContrast];

    /// Its token file.
    pub fn file(self) -> &'static str {
        match self {
            Self::Light => include_str!("../themes/light.toml"),
            Self::Dark => include_str!("../themes/dark.toml"),
            Self::HighContrast => include_str!("../themes/high-contrast.toml"),
        }
    }

    /// Its tokens, read from its file once.
    pub fn tokens(self) -> Tokens {
        static READ: OnceLock<[Tokens; 3]> = OnceLock::new();
        let read = READ.get_or_init(|| {
            Self::ALL.map(|theme| {
                parse(theme.file()).unwrap_or_else(|error| {
                    // The tests read every built-in file (`the_built_in_themes_are_valid`).
                    unreachable!("the built-in theme {theme:?} is valid: {error}")
                })
            })
        });
        let at = match self {
            Self::Light => 0,
            Self::Dark => 1,
            Self::HighContrast => 2,
        };
        read[at].clone()
    }

    /// The theme `choice` shows: 시스템 설정 따르기 takes 고대비 while the system asks for high
    /// contrast (`os_high_contrast`), else 어둡게 while its own colors are dark (`os_dark`).
    pub fn of(choice: ThemeChoice, os_dark: bool, os_high_contrast: bool) -> Self {
        let system = choice == ThemeChoice::System;
        if choice == ThemeChoice::HighContrast || (system && os_high_contrast) {
            Self::HighContrast
        } else if choice == ThemeChoice::Dark || (system && os_dark) {
            Self::Dark
        } else {
            Self::Light
        }
    }
}

/// The tokens of the built-in theme `choice` shows ([`Builtin::of`]).
pub fn builtin(choice: ThemeChoice, os_dark: bool, os_high_contrast: bool) -> Tokens {
    Builtin::of(choice, os_dark, os_high_contrast).tokens()
}

/// The tallest a line of text is, as a share of its font size. The systems' fonts draw a line
/// about 1.2 (Segoe UI 1.33, Malgun Gothic 1.33) to 1.45 (Noto Sans CJK) times their size; the
/// screens reckon a line's room with the same share (`Theme.line`, which a test holds to
/// [`line`]).
pub const LINE: f32 = 1.5;
/// The least room a row of a list leaves above and below its line of text, together.
pub const ROW_PADDING: f32 = 4.0;

/// The room of a line of text of `size` logical pixels: [`LINE`] times it, in whole pixels up.
pub fn line(size: f32) -> f32 {
    (size * LINE).ceil()
}

/// The share of the theme's row height and gaps that `density` gives them, as a fraction: four
/// fifths, all, or five quarters.
pub fn density_share(density: Density) -> (u16, u16) {
    match density {
        Density::Compact => (4, 5),
        Density::Normal => (1, 1),
        Density::Roomy => (5, 4),
    }
}

/// The tokens of a theme at a density and a size of text (THEME-07), the two apart:
///
/// - `density` changes the row height and the gaps (`gap-xs` to `gap-l`) alone, each to its
///   [`density_share`], rounded to whole pixels;
/// - `font_scale` changes the font sizes alone (`small` to `title`), each to its
///   [`FontScale::percent`], rounded to whole pixels;
/// - and nothing else: the colors, the fonts' families, the corners, the icons, the dims and the
///   marks stay as the theme has them.
///
/// The one meeting point is the row's floor, so that no row cuts its text: the row height is
/// `max(the density's row height, line(body) + ROW_PADDING)`, where `line(body)` is
/// `ceil(1.5 × the scaled body size)` ([`line`]) and [`ROW_PADDING`] 4 px. It depends on the size
/// of the text alone, and a row is taller than its density's only where its text needs it (115 %
/// and 130 % at 촘촘하게). Every result stays within the bounds a token file may give
/// ([`FONT_SIZES`], [`GAPS`], [`ROW_HEIGHTS`]). At 보통 and 100 % a built-in theme comes back as it
/// went in.
pub fn scaled(mut tokens: Tokens, density: Density, font_scale: FontScale) -> Tokens {
    // `size` times `over` / `under`, in whole pixels within `bounds`; a share of 1 leaves a size
    // as it is, also one with a fraction (a theme file may give one). Whole numbers keep the
    // product exact (130 / 100, not the float 1.3, which makes 15 px 19.4999 px).
    let times = |size: f32, (over, under): (u16, u16), bounds: &RangeInclusive<f32>| {
        if over == under {
            size
        } else {
            (size * f32::from(over) / f32::from(under))
                .round()
                .clamp(*bounds.start(), *bounds.end())
        }
    };
    let share = density_share(density);
    let size = &mut tokens.size;
    for gap in [
        &mut size.gap_xs,
        &mut size.gap_s,
        &mut size.gap_m,
        &mut size.gap_l,
    ] {
        *gap = times(*gap, share, &GAPS);
    }
    let row = times(size.row_height, share, &ROW_HEIGHTS);
    let percent = (font_scale.percent(), 100);
    let font = &mut tokens.font;
    for text in [
        &mut font.small,
        &mut font.note,
        &mut font.body,
        &mut font.app_name,
        &mut font.heading,
        &mut font.title,
    ] {
        *text = times(*text, percent, &FONT_SIZES);
    }
    let floor = line(tokens.font.body) + ROW_PADDING;
    tokens.size.row_height = row.max(floor).min(*ROW_HEIGHTS.end());
    tokens
}

/// The contrast ratio of the colors `a` and `b` as WCAG 2 counts it, from 1 (the same) to 21
/// (black and white). Their alpha is not counted.
pub fn contrast(a: Rgba, b: Rgba) -> f64 {
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

/// The relative luminance of `color` (WCAG 2).
fn luminance(color: Rgba) -> f64 {
    let channel = |value: u8| {
        let value = f64::from(value) / 255.0;
        if value <= 0.040_45 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(color.red) + 0.7152 * channel(color.green) + 0.0722 * channel(color.blue)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_built_in_themes_are_valid() {
        for theme in Builtin::ALL {
            if let Err(error) = parse(theme.file()) {
                panic!("{theme:?}: {error}");
            }
        }
        let keys = Builtin::Light.tokens().color.named().map(|(key, _)| key);
        assert_eq!(keys.as_slice(), COLOR_KEYS);
    }

    /// The colors and sizes `theme.slint` had before the tokens (its `out` properties, light
    /// and dark): the built-in light and dark themes must look exactly as before. The mixed
    /// colors are computed as Slint computed them then, with Slint's own `mix`.
    #[test]
    fn light_and_dark_keep_the_values_theme_slint_had() {
        let hex = |text: &str| color_of(text).expect("a color");
        let mix = |a: Rgba, b: Rgba, factor: f32| {
            let slint = |c: Rgba| slint::Color::from_argb_u8(c.alpha, c.red, c.green, c.blue);
            let mixed = slint(a).mix(&slint(b), factor);
            Rgba {
                red: mixed.red(),
                green: mixed.green(),
                blue: mixed.blue(),
                alpha: mixed.alpha(),
            }
        };
        let light = Builtin::Light.tokens();
        let dark = Builtin::Dark.tokens();
        let of = |colors: &Colors, key: &str| {
            colors
                .named()
                .into_iter()
                .find_map(|(name, color)| (name == key).then_some(color))
                .unwrap_or_else(|| panic!("{key}"))
        };
        // `root.dark ? <dark> : <light>` of each color, `warning: root.folder`, and the colors
        // the screens had as literals, the same in both (`toast-hover` and `toast-pressed`
        // were ToastButton's, `shadow` the menus', the hints' and the ring's, `shadow-small`
        // Segment's, `shadow-toast` Toast's, `shadow-dialog` DialogFrame's, `scrim` the
        // dialogs' and quick look's, `scrim-light` the one over the picker).
        for (key, light_was, dark_was) in [
            ("bg", "#F6F7F9", "#15171A"),
            ("panel", "#FFFFFF", "#1D2024"),
            ("raised", "#F0F2F5", "#24282D"),
            ("text", "#1C2024", "#E7E9EC"),
            ("muted", "#5C6370", "#A0A7B1"),
            ("faint", "#8A919C", "#7D848E"),
            // `disabled` was `faint` and `pressed` was `border` where the screens used them.
            ("disabled", "#8A919C", "#7D848E"),
            ("border", "#E1E4E8", "#2C3036"),
            ("pressed", "#E1E4E8", "#2C3036"),
            // The fields' selected text was `text` on `selection` (below).
            ("on-text-selection", "#1C2024", "#E7E9EC"),
            ("folder", "#B7791F", "#E0A84A"),
            ("warning", "#B7791F", "#E0A84A"),
            ("on-accent", "#FFFFFF", "#0E1A33"),
            ("toast-bg", "#1C2024", "#2C3138"),
            ("toast-text", "#F2F4F7", "#F2F4F7"),
            ("toast-link", "#9DBBFF", "#9DBBFF"),
            ("toast-hover", "#FFFFFF1F", "#FFFFFF1F"),
            ("toast-pressed", "#FFFFFF33", "#FFFFFF33"),
            ("shadow", "#00000033", "#00000033"),
            ("shadow-small", "#00000029", "#00000029"),
            ("shadow-toast", "#0000003D", "#0000003D"),
            ("shadow-dialog", "#00000052", "#00000052"),
            ("scrim", "#00000059", "#00000059"),
            ("scrim-light", "#00000033", "#00000033"),
        ] {
            assert_eq!(of(&light.color, key), hex(light_was), "light {key}");
            assert_eq!(of(&dark.color, key), hex(dark_was), "dark {key}");
        }
        // accent: `root.dark ? root.accent-base.mix(#FFFFFF, 0.65) : root.accent-base`.
        let (base, white) = (hex("#2F6FEB"), hex("#FFFFFF"));
        assert_eq!(light.color.accent, base);
        assert_eq!(dark.color.accent, mix(base, white, 0.65));
        // selection: `root.dark ? root.accent.mix(root.panel, 0.22)
        //                        : root.accent-base.mix(#FFFFFF, 0.12)`.
        assert_eq!(light.color.selection, mix(base, white, 0.12));
        let dark_selection = mix(mix(base, white, 0.65), hex("#1D2024"), 0.22);
        assert_eq!(dark.color.selection, dark_selection);
        for tokens in [&light, &dark] {
            assert_eq!(tokens.color.text_selection, tokens.color.selection);
        }
        // Every color is counted above: 24 by their values, the accent, the selection and the
        // fields' text selection.
        assert_eq!(light.color.named().len(), 27);
        for tokens in [&light, &dark] {
            // The sizes: theme.slint's, and the literals of the screens (13, 15 and 16 px).
            let f = &tokens.font;
            assert_eq!(
                (f.small, f.note, f.body, f.app_name, f.heading, f.title),
                (12.0, 13.0, 14.0, 15.0, 16.0, 20.0)
            );
            assert_eq!(
                (f.ui.as_str(), f.list.as_str(), f.mono.as_str()),
                ("", "", "")
            );
            let s = &tokens.size;
            assert_eq!(
                (s.gap_xs, s.gap_s, s.gap_m, s.gap_l),
                (4.0, 8.0, 12.0, 16.0)
            );
            assert_eq!((s.row_height, s.radius, s.radius_large), (32.0, 8.0, 12.0));
            // Thin icons and no shapes beside the colors, as before; cut items at 40 %, hidden
            // ones at half strength (the opacities the lists and the tree had).
            assert_eq!(tokens.icon_style, IconStyle::Line);
            assert!(!tokens.marks);
            assert_eq!((tokens.dim_cut, tokens.dim_hidden), (0.4, 0.5));
        }
        assert!(!light.dark);
        assert!(dark.dark);
    }

    /// The light theme's file with `from` replaced by `to` (once, and it must be there).
    fn light_with(from: &str, to: &str) -> String {
        let file = Builtin::Light.file();
        assert_eq!(file.matches(from).count(), 1, "{from}");
        file.replacen(from, to, 1)
    }

    #[test]
    fn a_file_that_lacks_a_key_is_refused_and_named() {
        let error = parse(&light_with("muted = \"#5C6370\"\n", "")).expect_err("refused");
        assert_eq!(error, ThemeError::Missing("color.muted".to_owned()));
        assert_eq!(error.key(), Some("color.muted"));
        let error = parse(&light_with("[size]", "[sizes]")).expect_err("refused");
        assert_eq!(error.key(), Some("sizes"), "{error}");
        let error = parse(&light_with("gap-xs = 4\n", "")).expect_err("refused");
        assert_eq!(error.key(), Some("size.gap-xs"));
    }

    #[test]
    fn a_key_no_theme_has_is_refused_and_named() {
        // A typo: the key it stands for is missing too, and the typo is what is told.
        let error = parse(&light_with("panel =", "pannel =")).expect_err("refused");
        assert_eq!(error, ThemeError::Unknown("color.pannel".to_owned()));
        let error =
            parse(&light_with("[font]", "dark-mode = true\n\n[font]")).expect_err("refused");
        assert_eq!(error.key(), Some("color.dark-mode"));
        let error =
            parse(&format!("{}\n[extra]\nx = 1\n", Builtin::Light.file())).expect_err("refused");
        assert_eq!(error, ThemeError::Unknown("extra".to_owned()));
    }

    #[test]
    fn a_color_that_is_not_one_is_refused_and_named() {
        for bad in [
            "\"#12345\"",
            "\"#1234567\"",
            "\"123456\"",
            "\"#GG0000\"",
            "\"red\"",
            "\"#123456789\"",
            "\"#１２３４５６\"",
            "0x123456",
            "\"\"",
        ] {
            let error = parse(&light_with(
                "accent = \"#2F6FEB\"",
                &format!("accent = {bad}"),
            ))
            .expect_err(bad);
            assert_eq!(error.key(), Some("color.accent"), "{bad}");
            assert!(
                matches!(error, ThemeError::Invalid { .. }),
                "{bad}: {error}"
            );
        }
        // Either case, and an alpha.
        let tokens = parse(&light_with(
            "accent = \"#2F6FEB\"",
            "accent = \"#2f6feb80\"",
        ))
        .expect("taken");
        assert_eq!(
            tokens.color.accent,
            Rgba {
                red: 0x2F,
                green: 0x6F,
                blue: 0xEB,
                alpha: 0x80
            }
        );
    }

    #[test]
    fn a_size_below_its_floor_or_past_its_cap_is_refused_and_named() {
        for (from, to, key) in [
            ("row-height = 32", "row-height = -32", "size.row-height"),
            ("row-height = 32", "row-height = 0", "size.row-height"),
            ("row-height = 32", "row-height = 15", "size.row-height"),
            ("row-height = 32", "row-height = 0.001", "size.row-height"),
            ("row-height = 32", "row-height = 129", "size.row-height"),
            ("row-height = 32", "row-height = nan", "size.row-height"),
            (
                "row-height = 32",
                "row-height = \"32px\"",
                "size.row-height",
            ),
            ("body = 14", "body = 400", "font.body"),
            ("body = 14", "body = -14.5", "font.body"),
            ("body = 14", "body = 7.5", "font.body"),
            ("small = 12", "small = 0.1", "font.small"),
            ("radius = 8", "radius = 1e9", "size.radius"),
            ("radius = 8", "radius = 0.5", "size.radius"),
            ("gap-l = 16", "gap-l = 65", "size.gap-l"),
            ("gap-xs = 4", "gap-xs = 0.2", "size.gap-xs"),
            ("dim-cut = 0.4", "dim-cut = 0", "theme.dim-cut"),
            ("dim-cut = 0.4", "dim-cut = 0.1", "theme.dim-cut"),
            ("dim-hidden = 0.5", "dim-hidden = 1.5", "theme.dim-hidden"),
            (
                "dim-hidden = 0.5",
                "dim-hidden = \"half\"",
                "theme.dim-hidden",
            ),
        ] {
            let error = parse(&light_with(from, to)).expect_err(to);
            assert_eq!(error.key(), Some(key), "{to}: {error}");
        }
        // A decimal size, the floors and the caps themselves are taken.
        let tokens = parse(&light_with("body = 14", "body = 13.5")).expect("taken");
        assert!((tokens.font.body - 13.5).abs() < f32::EPSILON);
        let tokens = parse(&light_with("row-height = 32", "row-height = 128")).expect("taken");
        assert!((tokens.size.row_height - ROW_HEIGHTS.end()).abs() < f32::EPSILON);
        let tokens = parse(&light_with("row-height = 32", "row-height = 16")).expect("taken");
        assert!((tokens.size.row_height - ROW_HEIGHTS.start()).abs() < f32::EPSILON);
        let tokens = parse(&light_with("small = 12", "small = 8")).expect("taken");
        assert!((tokens.font.small - FONT_SIZES.start()).abs() < f32::EPSILON);
        let tokens = parse(&light_with("dim-hidden = 0.5", "dim-hidden = 1")).expect("taken");
        assert!((tokens.dim_hidden - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn other_values_of_the_wrong_kind_are_refused_and_named() {
        for (from, to, key) in [
            ("scheme = \"light\"", "scheme = \"sepia\"", "theme.scheme"),
            (
                "icon-style = \"line\"",
                "icon-style = 2",
                "theme.icon-style",
            ),
            ("marks = false", "marks = \"no\"", "theme.marks"),
            ("ui = \"\"", "ui = 12", "font.ui"),
            ("mono = \"\"", "mono = \"Mono\\u0007\"", "font.mono"),
        ] {
            let error = parse(&light_with(from, to)).expect_err(to);
            assert_eq!(error.key(), Some(key), "{to}: {error}");
        }
        // A section that is a value.
        let file = Builtin::Light.file();
        let (head, rest) = file.split_at(file.find("[color]").expect("[color]"));
        let start = head.find("[theme]").expect("[theme]");
        let error = parse(&format!("{}theme = 1\n{rest}", &head[..start])).expect_err("refused");
        assert_eq!(error.key(), Some("theme"), "{error}");
        let long = "x".repeat(MAX_FONT_NAME + 1);
        let error =
            parse(&light_with("list = \"\"", &format!("list = \"{long}\""))).expect_err("too long");
        assert_eq!(error.key(), Some("font.list"));
        let tokens = parse(&light_with("list = \"\"", "list = \"Pretendard\"")).expect("taken");
        assert_eq!(tokens.font.list, "Pretendard");
        // Not TOML at all.
        let error = parse("[color\nbg = 1").expect_err("refused");
        assert!(matches!(error, ThemeError::Syntax(_)), "{error}");
        assert_eq!(error.key(), None);
        assert!(matches!(parse(""), Err(ThemeError::Missing(_))));
    }

    #[test]
    fn the_system_choice_follows_high_contrast_first_then_dark() {
        use ThemeChoice::{Dark, HighContrast, Light, System};
        for (choice, os_dark, os_high, shown) in [
            (System, false, false, Builtin::Light),
            (System, true, false, Builtin::Dark),
            (System, false, true, Builtin::HighContrast),
            (System, true, true, Builtin::HighContrast),
            (Light, true, true, Builtin::Light),
            (Dark, false, true, Builtin::Dark),
            (HighContrast, false, false, Builtin::HighContrast),
        ] {
            assert_eq!(Builtin::of(choice, os_dark, os_high), shown, "{choice:?}");
            assert_eq!(builtin(choice, os_dark, os_high), shown.tokens());
        }
    }

    /// The gaps and the row height of `tokens`.
    fn room(tokens: &Tokens) -> [f32; 5] {
        let s = &tokens.size;
        [s.gap_xs, s.gap_s, s.gap_m, s.gap_l, s.row_height]
    }

    /// The font sizes of `tokens`.
    fn text(tokens: &Tokens) -> [f32; 6] {
        let f = &tokens.font;
        [f.small, f.note, f.body, f.app_name, f.heading, f.title]
    }

    /// THEME-07: the density never changes a font size, the size of the text never changes a gap,
    /// and neither changes anything else; the row height is the density's, or the floor of its
    /// text where that is higher, and the floor follows the size of the text alone.
    #[test]
    fn the_density_and_the_size_of_the_text_are_apart() {
        for theme in Builtin::ALL {
            let tokens = theme.tokens();
            for density in Density::ALL {
                for scale in FontScale::ALL {
                    let what = format!("{theme:?} {density:?} {scale:?}");
                    let both = scaled(tokens.clone(), density, scale);
                    let text_alone = scaled(tokens.clone(), Density::Normal, scale);
                    let room_alone = scaled(tokens.clone(), density, FontScale::Normal);
                    assert_eq!(both.font, text_alone.font, "{what}: the text");
                    assert_eq!(room(&both)[..4], room(&room_alone)[..4], "{what}: the gaps");
                    let floor = line(both.font.body) + ROW_PADDING;
                    assert_eq!(
                        both.size.row_height,
                        room_alone.size.row_height.max(floor),
                        "{what}: the row"
                    );
                    // The text's floor is the same at every density.
                    for other in Density::ALL {
                        let at = scaled(tokens.clone(), other, scale);
                        assert_eq!(line(at.font.body), line(both.font.body), "{what}");
                    }
                    // What neither touches: the colors, the families, the corners, the icons,
                    // the dims and the marks.
                    let mut rest = both.clone();
                    rest.font.small = tokens.font.small;
                    rest.font.note = tokens.font.note;
                    rest.font.body = tokens.font.body;
                    rest.font.app_name = tokens.font.app_name;
                    rest.font.heading = tokens.font.heading;
                    rest.font.title = tokens.font.title;
                    rest.size.gap_xs = tokens.size.gap_xs;
                    rest.size.gap_s = tokens.size.gap_s;
                    rest.size.gap_m = tokens.size.gap_m;
                    rest.size.gap_l = tokens.size.gap_l;
                    rest.size.row_height = tokens.size.row_height;
                    assert_eq!(rest, tokens, "{what}: the rest");
                    // A density changes the row and the gaps of the theme, a size its text, at
                    // every step but the normal one.
                    if density != Density::Normal {
                        assert_ne!(room(&room_alone)[..4], room(&tokens)[..4], "{what}");
                    }
                    if scale != FontScale::Normal {
                        assert_ne!(text(&text_alone), text(&tokens), "{what}");
                    }
                }
            }
            // 보통 and 100 %: the theme as it is.
            assert_eq!(
                scaled(tokens.clone(), Density::Normal, FontScale::Normal),
                tokens
            );
        }
    }

    /// The two ends: 촘촘하게 at 90 % and 넉넉하게 at 130 %, of the light theme (dark and high
    /// contrast have the same sizes).
    #[test]
    fn the_smallest_and_the_largest_looks_have_these_sizes() {
        let light = Builtin::Light.tokens();
        let smallest = scaled(light.clone(), Density::Compact, FontScale::Smaller);
        // small 12, note 13, body 14, app-name 15, heading 16, title 20 at 90 %, rounded.
        assert_eq!(text(&smallest), [11.0, 12.0, 13.0, 14.0, 14.0, 18.0]);
        // gap-xs 4, s 8, m 12, l 16 and the row 32 at four fifths, rounded.
        assert_eq!(room(&smallest), [3.0, 6.0, 10.0, 13.0, 26.0]);
        let largest = scaled(light.clone(), Density::Roomy, FontScale::Largest);
        assert_eq!(text(&largest), [16.0, 17.0, 18.0, 20.0, 21.0, 26.0]);
        assert_eq!(room(&largest), [5.0, 10.0, 15.0, 20.0, 40.0]);
        // And the steps between.
        let larger = scaled(light.clone(), Density::Normal, FontScale::Larger);
        assert_eq!(text(&larger), [14.0, 15.0, 16.0, 17.0, 18.0, 23.0]);
        assert_eq!(room(&larger), room(&light));
        for theme in [Builtin::Dark, Builtin::HighContrast] {
            let tokens = theme.tokens();
            for (density, scale, want) in [
                (Density::Compact, FontScale::Smaller, &smallest),
                (Density::Roomy, FontScale::Largest, &largest),
            ] {
                let got = scaled(tokens.clone(), density, scale);
                assert_eq!(
                    (text(&got), room(&got)),
                    (text(want), room(want)),
                    "{theme:?}"
                );
            }
        }
    }

    /// No row cuts its line of text: the row height is at least `ceil(1.5 × body) + 4`, at every
    /// look and for a theme whose rows are lower than its text, within the bounds of a token file.
    #[test]
    fn a_row_is_never_lower_than_its_line_of_text() {
        assert_eq!(
            (line(13.0), line(14.0), line(16.0), line(18.0)),
            (20.0, 21.0, 24.0, 27.0)
        );
        let light = Builtin::Light.tokens();
        // 촘촘하게 is 26 px, which its own text fits (21 + 4) and 90 % too (20 + 4); 16 px text
        // takes 28 px (24 + 4), 18 px text 31 px (27 + 4).
        for (scale, row) in [
            (FontScale::Smaller, 26.0),
            (FontScale::Normal, 26.0),
            (FontScale::Larger, 28.0),
            (FontScale::Largest, 31.0),
        ] {
            let got = scaled(light.clone(), Density::Compact, scale);
            assert_eq!(got.size.row_height, row, "{scale:?}");
        }
        for theme in Builtin::ALL {
            for density in Density::ALL {
                for scale in FontScale::ALL {
                    let got = scaled(theme.tokens(), density, scale);
                    assert!(
                        got.size.row_height >= line(got.font.body) + ROW_PADDING,
                        "{theme:?} {density:?} {scale:?}"
                    );
                }
            }
        }
        // A theme whose rows are lower than its text gets the floor at 보통 and 100 % too.
        let low = parse(&light_with("row-height = 32", "row-height = 16")).expect("taken");
        assert_eq!(
            scaled(low, Density::Normal, FontScale::Normal)
                .size
                .row_height,
            25.0
        );
        // The largest text a file may give stays within its bound at 130 %, and so does its row.
        let huge = parse(&light_with("body = 14", "body = 72")).expect("taken");
        let got = scaled(huge, Density::Compact, FontScale::Largest);
        assert_eq!(got.font.body, *FONT_SIZES.end());
        assert_eq!(got.size.row_height, 112.0);
        let tall = parse(&light_with("row-height = 32", "row-height = 128")).expect("taken");
        let got = scaled(tall, Density::Roomy, FontScale::Normal);
        assert_eq!(got.size.row_height, *ROW_HEIGHTS.end());
        // The smallest gap a file may give stays 1 px at 촘촘하게.
        let thin = parse(&light_with("gap-xs = 4", "gap-xs = 1")).expect("taken");
        let got = scaled(thin, Density::Compact, FontScale::Normal);
        assert_eq!(got.size.gap_xs, *GAPS.start());
    }

    #[test]
    fn high_contrast_keeps_text_at_seven_to_one_or_more() {
        let tokens = Builtin::HighContrast.tokens();
        let c = &tokens.color;
        assert_eq!(c.bg, Rgba::rgb(0, 0, 0), "a black background");
        assert_eq!(c.text, Rgba::rgb(255, 255, 255), "white text");
        // A yellow: red and green high, blue low.
        assert!(c.accent.red >= 0xE0 && c.accent.green >= 0xC0 && c.accent.blue <= 0x40);
        for (what, front, back, least) in [
            ("text on the window", c.text, c.bg, 7.0),
            ("text on a pane", c.text, c.panel, 7.0),
            ("text under the pointer", c.text, c.raised, 7.0),
            ("muted text", c.muted, c.panel, 7.0),
            ("on the accent", c.on_accent, c.accent, 7.0),
            ("a selected item's text", c.text, c.selection, 7.0),
            ("a selected item's muted text", c.muted, c.selection, 7.0),
            ("a selected item's faint text", c.faint, c.selection, 7.0),
            (
                "text on a pressed button or a hovered tab",
                c.text,
                c.pressed,
                7.0,
            ),
            ("muted text on a hovered segment", c.muted, c.pressed, 7.0),
            ("a pressed row's hint", c.faint, c.pressed, 7.0),
            ("a hint under the pointer", c.faint, c.raised, 7.0),
            (
                "the selected text in a field",
                c.on_text_selection,
                c.text_selection,
                7.0,
            ),
            ("a field's text selection", c.text_selection, c.bg, 3.0),
            ("a notice", c.toast_text, c.toast_bg, 7.0),
            ("a notice's action", c.toast_link, c.toast_bg, 7.0),
            (
                "a notice's pressed action",
                c.toast_link,
                over(c.toast_pressed, c.toast_bg),
                7.0,
            ),
            ("the accent's ring and drop frame", c.accent, c.panel, 7.0),
            ("hints", c.faint, c.panel, 7.0),
            ("what cannot be used", c.disabled, c.panel, 4.5),
            ("the folders' icons", c.folder, c.panel, 3.0),
            ("a warning's icon", c.warning, c.panel, 3.0),
            ("lines and frames", c.border, c.panel, 3.0),
        ] {
            let ratio = contrast(front, back);
            assert!(ratio >= least, "{what}: {ratio:.2}:1 < {least}:1");
        }
        // What cannot be used is a clear step down from what can (the dashed frame tells it
        // too): at least as far from the text as in the dark theme.
        let step = contrast(c.text, c.disabled);
        let dark = Builtin::Dark.tokens();
        let dark_step = contrast(dark.color.text, dark.color.disabled);
        assert!(
            step >= 3.0 && step >= dark_step,
            "{step:.2}:1, dark {dark_step:.2}:1"
        );
        assert!(tokens.dark, "the system's widgets are drawn dark on it");
        assert!(tokens.marks, "shapes mark what the colors tell");
        assert_eq!(tokens.icon_style, IconStyle::Bold);
    }

    /// In high contrast a cut and a hidden item stay readable at their strength wherever they
    /// show: in the list and the grid their name and columns, on the pane, selected and under
    /// the pointer; in the sidebar a hidden folder's name, a drive's note and the faint name and
    /// note of one a filter dims, pressed too. One pair stays under 7:1, only while the pointer
    /// presses the row; it is named here with its ratio.
    #[test]
    fn high_contrast_keeps_cut_and_hidden_items_readable_wherever_they_show() {
        const PRESSED_ONLY: &[(&str, f64)] = &[("hidden folder's faint text on pressed", 6.05)];
        let tokens = Builtin::HighContrast.tokens();
        let c = &tokens.color;
        let list = [
            ("pane", c.panel),
            ("selection", c.selection),
            ("raised", c.raised),
        ];
        let sidebar = [
            ("pane", c.panel),
            ("selection", c.selection),
            ("raised", c.raised),
            ("pressed", c.pressed),
        ];
        let item = [("name", c.text), ("columns", c.muted)];
        let folder = [
            ("name", c.text),
            ("muted text", c.muted),
            ("faint text", c.faint),
        ];
        let mut under = Vec::new();
        for (what, dim, fronts, backs) in [
            ("cut item's", tokens.dim_cut, &item[..], &list[..]),
            ("hidden item's", tokens.dim_hidden, &item[..], &list[..]),
            (
                "hidden folder's",
                tokens.dim_hidden,
                &folder[..],
                &sidebar[..],
            ),
        ] {
            for (name, front) in fronts {
                for (on, back) in backs {
                    let ratio = contrast(faded(*front, *back, dim), *back);
                    if ratio < 7.0 {
                        under.push((format!("{what} {name} on {on}"), ratio));
                    }
                }
            }
        }
        assert_eq!(
            under
                .iter()
                .map(|(what, _)| what.as_str())
                .collect::<Vec<_>>(),
            PRESSED_ONLY
                .iter()
                .map(|(what, _)| *what)
                .collect::<Vec<_>>(),
            "{under:?}"
        );
        for ((what, ratio), (_, listed)) in under.iter().zip(PRESSED_ONLY) {
            assert!(
                *ratio >= 4.5 && (ratio - listed).abs() < 0.01,
                "{what}: {ratio:.2}:1"
            );
        }
    }

    /// `front` drawn at `opacity` over `back`, as the screens blend a dimmed item.
    fn faded(front: Rgba, back: Rgba, opacity: f32) -> Rgba {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "a mix of two channels stays within 0..=255"
        )]
        let channel =
            |f: u8, b: u8| (f32::from(f) * opacity + f32::from(b) * (1.0 - opacity)).round() as u8;
        Rgba::rgb(
            channel(front.red, back.red),
            channel(front.green, back.green),
            channel(front.blue, back.blue),
        )
    }

    /// The translucent color `top` laid over `back`.
    fn over(top: Rgba, back: Rgba) -> Rgba {
        faded(
            Rgba::rgb(top.red, top.green, top.blue),
            back,
            f32::from(top.alpha) / 255.0,
        )
    }

    /// The light and dark themes dim cut and hidden items as before (their contrast is what it
    /// was; only the high contrast theme promises 7:1 for them).
    #[test]
    fn light_and_dark_dim_cut_and_hidden_items_as_before() {
        // Name and columns × cut and hidden × on the pane and selected: the ratios of 0.4.0,
        // less 0.05 for the rounding of the blend.
        let before = [
            (
                Builtin::Light,
                [2.39, 2.32, 3.15, 3.05, 1.75, 1.69, 2.09, 1.99],
            ),
            (
                Builtin::Dark,
                [3.24, 2.78, 4.31, 3.49, 2.15, 1.88, 2.68, 2.22],
            ),
        ];
        for (theme, floors) in before {
            let tokens = theme.tokens();
            let c = &tokens.color;
            let mut at = 0;
            for front in [c.text, c.muted] {
                for dim in [tokens.dim_cut, tokens.dim_hidden] {
                    for back in [c.panel, c.selection] {
                        let ratio = contrast(faded(front, back, dim), back);
                        assert!(ratio >= floors[at], "{theme:?} {at}: {ratio:.3}");
                        at += 1;
                    }
                }
            }
        }
    }

    /// The light and dark themes show the selected text in a field as before (`text` on
    /// `selection`, with their ratios); high contrast gives it the accent.
    #[test]
    fn light_and_dark_select_text_in_fields_as_before() {
        // The text on the selection, and the selection against the field: the ratios of 0.4.0,
        // less 0.01.
        for (theme, on, against) in [(Builtin::Light, 14.05, 1.08), (Builtin::Dark, 9.10, 1.62)] {
            let c = theme.tokens().color;
            let ratio = contrast(c.on_text_selection, c.text_selection);
            assert!(ratio >= on, "{theme:?}: {ratio:.3}");
            let ratio = contrast(c.text_selection, c.bg);
            assert!(ratio >= against, "{theme:?}: {ratio:.3}");
        }
        let c = Builtin::HighContrast.tokens().color;
        assert_eq!(
            (c.text_selection, c.on_text_selection),
            (c.accent, c.on_accent)
        );
    }

    /// Every field draws its selected text with the tokens for it, so that a theme sets it in
    /// one place.
    #[test]
    fn the_fields_take_their_text_selection_from_the_theme() {
        let setting = regex::Regex::new(r"selection-(background|foreground)-color\s*:\s*([^;]*);")
            .expect("a pattern");
        let folder = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("ui");
        let mut seen = 0;
        for entry in std::fs::read_dir(&folder).expect("the screens' folder") {
            let path = entry.expect("an entry").path();
            if path
                .extension()
                .is_none_or(|extension| extension != "slint")
            {
                continue;
            }
            let text = std::fs::read_to_string(&path).expect("a .slint file");
            for found in setting.captures_iter(&code_of(&text)) {
                let want = if &found[1] == "background" {
                    "Theme.text-selection"
                } else {
                    "Theme.on-text-selection"
                };
                assert_eq!(found[2].trim(), want, "{}", path.display());
                seen += 1;
            }
        }
        // The rename and pattern dialogs, the path field and `Field`, two colors each.
        assert!(seen >= 8, "{seen}");
    }

    /// `text` with its string literals and comments blanked out, for the look below.
    fn code_of(text: &str) -> String {
        let mut code = String::with_capacity(text.len());
        let mut chars = text.chars().peekable();
        let (mut in_string, mut in_line_comment, mut in_block_comment) = (false, false, false);
        while let Some(c) = chars.next() {
            if in_line_comment {
                if c == '\n' {
                    in_line_comment = false;
                    code.push(c);
                }
            } else if in_block_comment {
                if c == '*' && chars.peek() == Some(&'/') {
                    chars.next();
                    in_block_comment = false;
                } else if c == '\n' {
                    code.push(c);
                }
            } else if in_string {
                match c {
                    '\\' => {
                        chars.next();
                    }
                    '"' => {
                        in_string = false;
                        code.push(c);
                    }
                    _ => {}
                }
            } else if c == '"' {
                in_string = true;
                code.push(c);
            } else if c == '/' && chars.peek() == Some(&'/') {
                in_line_comment = true;
            } else if c == '/' && chars.peek() == Some(&'*') {
                chars.next();
                in_block_comment = true;
            } else {
                code.push(c);
            }
        }
        code
    }

    /// The tokens are the source of the screens' text sizes and colors (the plan's "테마 토큰이
    /// 화면 크기의 원본"): no .slint file but theme.slint writes a length in a font size (in any
    /// unit, also in one branch of a condition) or a color of its own (hex, `rgb()`, `rgba()`,
    /// `hsv()`, `Colors.…`). A literal that had to stay would be named here with its reason;
    /// none does.
    #[test]
    fn the_screens_take_their_font_sizes_and_colors_from_the_theme() {
        const ALLOWED: &[(&str, &str, &str)] = &[];
        // A length literal anywhere in the value of a `font-size` (up to its `;`).
        let size = regex::Regex::new(r"font-size\s*:[^;]*?[0-9.]+\s*(px|phx|pt|rem|cm|mm|in)\b")
            .expect("a pattern");
        let color = regex::Regex::new(
            r"#[0-9A-Fa-f]{3,8}\b|(^|[^\w-])(rgb|rgba|argb|hsv|hsva)\s*\(|\bColors\.",
        )
        .expect("a pattern");
        let folder = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("ui");
        let mut files: Vec<_> = std::fs::read_dir(&folder)
            .expect("the screens' folder")
            .map(|entry| entry.expect("an entry").path())
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "slint")
            })
            .collect();
        files.sort();
        assert!(files.len() > 15, "{files:?}");
        let mut found = Vec::new();
        for path in files {
            let name = path
                .file_name()
                .expect("a name")
                .to_string_lossy()
                .into_owned();
            if name == "theme.slint" {
                continue;
            }
            let text = std::fs::read_to_string(&path).expect("a .slint file");
            for (number, line) in code_of(&text).lines().enumerate() {
                for literal in size.find_iter(line).chain(color.find_iter(line)) {
                    let allowed = ALLOWED
                        .iter()
                        .any(|(file, what, _reason)| *file == name && *what == literal.as_str());
                    if !allowed {
                        found.push(format!("{name}:{}: {}", number + 1, literal.as_str()));
                    }
                }
            }
        }
        assert_eq!(found, Vec::<String>::new());
        // The look itself finds them, and skips what only a string or a comment says and the
        // tokens' own names.
        let sample = "a { font-size: 13px; c: #FFFFFF33; }\n// #123456 font-size: 1px\n\
                      t: \"#ABCDEF\";\n\
                      b { font-size: x ? 16px : Theme.font-body; d: rgb(1, 2, 3); }\n\
                      c { font-size: 1.1rem; e: Colors.red; f: hsv(1, 1, 1); g: rgba(0,0,0,1); }\n\
                      h { font-size: 10pt; default-font-size: 12phx; }\n\
                      i { font-size: Theme.font-small; color: Theme.text; v: x.to-hsv().value; }";
        let code = code_of(sample);
        assert_eq!(size.find_iter(&code).count(), 5, "{code}");
        assert_eq!(color.find_iter(&code).count(), 5, "{code}");
    }

    #[test]
    fn contrast_is_as_wcag_counts_it() {
        let (black, white) = (Rgba::rgb(0, 0, 0), Rgba::rgb(255, 255, 255));
        assert!((contrast(black, white) - 21.0).abs() < 1e-9);
        assert!((contrast(white, black) - 21.0).abs() < 1e-9);
        assert!((contrast(white, white) - 1.0).abs() < 1e-9);
        // WCAG's own example: #767676 on white is 4.54:1.
        let grey = Rgba::rgb(0x76, 0x76, 0x76);
        assert!((contrast(grey, white) - 4.54).abs() < 0.01);
    }
}
