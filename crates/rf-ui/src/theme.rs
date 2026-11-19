//! The look of the app as design tokens (THEME-01, THEME-03): its colors, fonts, sizes, icon
//! style and whether shapes also mark what the colors tell, read from a TOML file. The three
//! built-in themes, 밝게, 어둡게 and 고대비, are the files `crates/rf-ui/themes/*.toml`, built into
//! the program: no theme file is read from the disk. `docs/ux/themes.md` lists the tokens and
//! what each means.
//!
//! [`parse`] takes a file only whole: every key must be there, each with a value of its kind
//! (a color `#RRGGBB` or `#RRGGBBAA`, a size above 0 and under its cap, a font name, one of a
//! few words, true or false), and a key it does not know is refused too, so that a typo shows
//! instead of leaving a token at some default. The error names the key. A file that is refused
//! changes nothing: the theme shown stays (THEME-05's rule, for the import that comes later).
//!
//! [`builtin`] picks the theme of a [`ThemeChoice`]: 시스템 설정 따르기 takes 고대비 while the
//! system asks for high contrast, else 어둡게 or 밝게 as the system's own colors are.

use std::sync::OnceLock;

use crate::settings::ThemeChoice;

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
    /// What cannot be used now.
    pub faint: Rgba,
    /// Lines between parts, and frames.
    pub border: Rgba,
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
    pub fn named(&self) -> [(&'static str, Rgba); 23] {
        [
            ("bg", self.bg),
            ("panel", self.panel),
            ("raised", self.raised),
            ("text", self.text),
            ("muted", self.muted),
            ("faint", self.faint),
            ("border", self.border),
            ("folder", self.folder),
            ("warning", self.warning),
            ("accent", self.accent),
            ("on-accent", self.on_accent),
            ("selection", self.selection),
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
    /// a heavier frame around the chosen one of a set, a frame around a dialog (고대비).
    pub marks: bool,
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
const THEME_KEYS: &[&str] = &["scheme", "icon-style", "marks"];
/// The keys of `[color]`.
const COLOR_KEYS: &[&str] = &[
    "bg",
    "panel",
    "raised",
    "text",
    "muted",
    "faint",
    "border",
    "folder",
    "warning",
    "accent",
    "on-accent",
    "selection",
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

/// The largest font size a theme may give, in logical pixels.
pub const MAX_FONT: f32 = 72.0;
/// The largest gap.
pub const MAX_GAP: f32 = 64.0;
/// The largest row height.
pub const MAX_ROW_HEIGHT: f32 = 128.0;
/// The largest corner radius.
pub const MAX_RADIUS: f32 = 64.0;
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
        color: Colors {
            bg: color.color("bg")?,
            panel: color.color("panel")?,
            raised: color.color("raised")?,
            text: color.color("text")?,
            muted: color.color("muted")?,
            faint: color.color("faint")?,
            border: color.color("border")?,
            folder: color.color("folder")?,
            warning: color.color("warning")?,
            accent: color.color("accent")?,
            on_accent: color.color("on-accent")?,
            selection: color.color("selection")?,
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
            small: font.size("small", MAX_FONT)?,
            note: font.size("note", MAX_FONT)?,
            body: font.size("body", MAX_FONT)?,
            app_name: font.size("app-name", MAX_FONT)?,
            heading: font.size("heading", MAX_FONT)?,
            title: font.size("title", MAX_FONT)?,
        },
        size: Sizes {
            gap_xs: size.size("gap-xs", MAX_GAP)?,
            gap_s: size.size("gap-s", MAX_GAP)?,
            gap_m: size.size("gap-m", MAX_GAP)?,
            gap_l: size.size("gap-l", MAX_GAP)?,
            row_height: size.size("row-height", MAX_ROW_HEIGHT)?,
            radius: size.size("radius", MAX_RADIUS)?,
            radius_large: size.size("radius-large", MAX_RADIUS)?,
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

    /// A size in logical pixels, above 0 and at most `max`: a whole number or a decimal one.
    fn size(&mut self, key: &str, max: f32) -> Result<f32, ThemeError> {
        let value = self.value(key)?;
        #[expect(
            clippy::cast_precision_loss,
            clippy::cast_possible_truncation,
            reason = "a size is checked against a cap far below where f32 loses whole pixels"
        )]
        let size = match value {
            toml::Value::Integer(number) => Some(number as f32),
            toml::Value::Float(number) => Some(number as f32),
            _ => None,
        };
        size.filter(|size| size.is_finite() && *size > 0.0 && *size <= max)
            .ok_or_else(|| self.invalid(key, format!("a size above 0 and at most {max}")))
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
            ("border", "#E1E4E8", "#2C3036"),
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
        // Every color is counted above: 21 by their values, the accent and the selection.
        assert_eq!(light.color.named().len(), 23);
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
            // Thin icons and no shapes beside the colors, as before.
            assert_eq!(tokens.icon_style, IconStyle::Line);
            assert!(!tokens.marks);
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
    fn a_size_below_zero_zero_or_past_its_cap_is_refused_and_named() {
        for (from, to, key) in [
            ("row-height = 32", "row-height = -32", "size.row-height"),
            ("row-height = 32", "row-height = 0", "size.row-height"),
            ("row-height = 32", "row-height = 129", "size.row-height"),
            ("row-height = 32", "row-height = nan", "size.row-height"),
            (
                "row-height = 32",
                "row-height = \"32px\"",
                "size.row-height",
            ),
            ("body = 14", "body = 400", "font.body"),
            ("body = 14", "body = -14.5", "font.body"),
            ("radius = 8", "radius = 1e9", "size.radius"),
            ("gap-l = 16", "gap-l = 65", "size.gap-l"),
        ] {
            let error = parse(&light_with(from, to)).expect_err(to);
            assert_eq!(error.key(), Some(key), "{to}: {error}");
        }
        // A decimal size and the caps themselves are taken.
        let tokens = parse(&light_with("body = 14", "body = 13.5")).expect("taken");
        assert!((tokens.font.body - 13.5).abs() < f32::EPSILON);
        let tokens = parse(&light_with("row-height = 32", "row-height = 128")).expect("taken");
        assert!((tokens.size.row_height - MAX_ROW_HEIGHT).abs() < f32::EPSILON);
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
            ("a notice", c.toast_text, c.toast_bg, 7.0),
            ("a notice's action", c.toast_link, c.toast_bg, 7.0),
            ("the accent's ring and drop frame", c.accent, c.panel, 7.0),
            ("what cannot be used", c.faint, c.panel, 4.5),
            ("the folders' icons", c.folder, c.panel, 3.0),
            ("a warning's icon", c.warning, c.panel, 3.0),
            ("lines and frames", c.border, c.panel, 3.0),
            ("a disabled button's text", c.faint, c.border, 3.0),
        ] {
            let ratio = contrast(front, back);
            assert!(ratio >= least, "{what}: {ratio:.2}:1 < {least}:1");
        }
        assert!(tokens.dark, "the system's widgets are drawn dark on it");
        assert!(tokens.marks, "shapes mark what the colors tell");
        assert_eq!(tokens.icon_style, IconStyle::Bold);
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
    /// 화면 크기의 원본"): no .slint file but theme.slint writes a font size in pixels or a
    /// color in hex. A literal that had to stay would be named here with its reason; none does.
    #[test]
    fn the_screens_take_their_font_sizes_and_colors_from_the_theme() {
        const ALLOWED: &[(&str, &str, &str)] = &[];
        let size = regex::Regex::new(r"font-size\s*:\s*-?[0-9.]+\s*px").expect("a pattern");
        let color = regex::Regex::new(r"#[0-9A-Fa-f]{3,8}\b").expect("a pattern");
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
        // The look itself finds them, and skips what only a string or a comment says.
        let sample =
            "a { font-size: 13px; c: #FFFFFF33; }\n// #123456 font-size: 1px\nt: \"#ABCDEF\";";
        let code = code_of(sample);
        assert_eq!(size.find_iter(&code).count(), 1);
        assert_eq!(color.find_iter(&code).count(), 1);
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
