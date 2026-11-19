//! The words of the app in Korean and English (I18N-01·02), from the Project Fluent files in
//! `crates/rf-ui/i18n/<language>/<area>.ftl`. The build embeds them: nothing is read from the
//! disk at run time, and each language is parsed once, when it is first asked for.
//!
//! The language is chosen once, as the app starts ([`init`] with [`start_language`]): the
//! settings' `language`, or the system's ([`pick`]). Changing it takes a restart (a change
//! without one is I18N-01's P1 part). Before [`init`], and so in every test (no test calls it),
//! the words are Korean; a test of the English words makes a [`Localizer`] of its own.
//!
//! How the messages are written, for the code that asks for them (`words.rs`, `format.rs`):
//!
//! - A key is `area-meaning` (`job-done`, `status-items`, `error-not-found`), the same in every
//!   language. A key a language lacks shows in English, and one no language has shows as the key
//!   itself ([`Localizer::text`]); a debug build notes either on stderr.
//! - Numbers and names are arguments (`{ $count }`, `{ $name }`). A number the screen shows in a
//!   form of its own (a size through `format_size`, grouped digits) is passed as the text it
//!   shows; a count is passed as a number ([`count`], [`number`]), which shows as Rust shows it
//!   and gives English its plural (`{ $count -> [one] 1 item *[other] { $count } items }`).
//! - A sentence is one message in each language, never put together from pieces of another:
//!   what a job did is a selector on its work (`{ $work -> [copy] … *[other] … }`).
//! - A Korean particle that follows how a name is read (이/가) is chosen by the Korean message
//!   from what Rust tells of the name, in an argument whose name starts with `ko-`
//!   (`$ko-final`: `yes` when it ends in a final consonant, else `no`): only the Korean messages
//!   use such arguments, and Rust holds no Korean word.
//! - Every message may use `$os` ([`OS_ARG`]: `windows`, `macos` or `other`), the system the app
//!   runs on, which the caller need not pass: English says "Recycle Bin" on Windows and "Trash"
//!   elsewhere where Korean says 휴지통 on both. A caller that names a system passes `os` itself.
//! - A function that returns `&'static str` asks [`t_static`], for a message without arguments
//!   (other than `$os`).
//! - The screens' words without arguments (menus, buttons, labels, accessible names) are those of
//!   `ui.ftl`: each key is a property of the Slint global `Tr` (`ui/tr.slint`), which
//!   [`crate::ui_bridge::speak`] fills as each window is made. A screen's words with a name or a
//!   key in them are made in Rust from `screen.ftl` and given to the window.

use std::collections::HashMap;
use std::fmt;
use std::sync::{LazyLock, Mutex, OnceLock, PoisonError};

use fluent_bundle::concurrent::FluentBundle;
use fluent_bundle::{FluentArgs, FluentError, FluentResource, FluentValue};
use rf_core::Os;
use unic_langid::LanguageIdentifier;

use crate::settings::{self, LanguageChoice};

/// A language the app speaks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Lang {
    /// 한국어, the language of the app until [`init`] says otherwise.
    #[default]
    Ko,
    /// English, which also shows a message the other language lacks (I18N-02).
    En,
}

impl Lang {
    /// Every language, Korean first.
    pub const ALL: [Lang; 2] = [Lang::Ko, Lang::En];

    /// The language's code, as the folder of its files is named: `ko`, `en`.
    pub fn code(self) -> &'static str {
        match self {
            Lang::Ko => "ko",
            Lang::En => "en",
        }
    }

    /// The Fluent files of the language, in the order of [`AREAS`].
    fn sources(self) -> impl Iterator<Item = (&'static str, &'static str)> {
        AREAS.iter().map(move |&(area, ko, en)| match self {
            Lang::Ko => (area, ko),
            Lang::En => (area, en),
        })
    }
}

/// The language a run speaks: the one the settings name, or with [`LanguageChoice::System`]
/// the system's, `os_locale` (`ko-KR`, `ko_KR.UTF-8`, `en-US`; `None` when the system does not
/// say): Korean for any Korean locale, English for every other one and for none.
pub fn pick(setting: LanguageChoice, os_locale: Option<&str>) -> Lang {
    match setting {
        LanguageChoice::Ko => Lang::Ko,
        LanguageChoice::En => Lang::En,
        LanguageChoice::System => match os_locale {
            Some(locale) if is_korean(locale) => Lang::Ko,
            _ => Lang::En,
        },
    }
}

/// Whether the locale `locale` is a Korean one: its language, before the first `-`, `_`, `.` or
/// `@`, is `ko` in any case.
fn is_korean(locale: &str) -> bool {
    locale
        .trim()
        .split(['-', '_', '.', '@'])
        .next()
        .is_some_and(|language| language.eq_ignore_ascii_case("ko"))
}

/// The language of this run, as the app starts: the settings' `language` ([`pick`]), else the
/// system's. The settings file is only read here ([`settings::peek_language`]): one that cannot
/// be used is set aside by [`settings::load`] alone, once this instance holds its lock.
pub fn start_language() -> Lang {
    let choice = rf_core::AppPaths::detect()
        .map(|paths| settings::peek_language(paths.config_dir()))
        .unwrap_or_default();
    pick(choice, sys_locale::get_locale().as_deref())
}

/// The name of the argument every message may use for the system the app runs on (`windows`,
/// `macos` or `other`; [`os_arg`]).
pub const OS_ARG: &str = "os";

/// `os` as the messages' `$os` selects it: `windows`, `macos` or `other`.
pub fn os_arg(os: Os) -> FluentValue<'static> {
    FluentValue::from(match os {
        Os::Windows => "windows",
        Os::MacOs => "macos",
        Os::Other => "other",
    })
}

/// The largest whole number a Fluent number holds exactly, 2^53.
const EXACT: u64 = 1 << 53;

/// `n` as a number argument, which shows as Rust shows it (`12`) and selects English's plural.
/// One beyond what a Fluent number holds exactly (2^53) is passed as its text: it shows whole,
/// and selects the `other` plural, which is its own.
pub fn number(n: u64) -> FluentValue<'static> {
    if n <= EXACT {
        FluentValue::from(n)
    } else {
        FluentValue::from(n.to_string())
    }
}

/// A count of things as a number argument ([`number`]).
pub fn count(n: usize) -> FluentValue<'static> {
    number(u64::try_from(n).unwrap_or(u64::MAX))
}

/// An argument of a message: its name and its value.
pub type Arg<'a> = (&'a str, FluentValue<'a>);

/// The Fluent files of each area of the app: its name, then its Korean and its English text.
macro_rules! area {
    ($name:literal) => {
        (
            $name,
            include_str!(concat!("../i18n/ko/", $name, ".ftl")),
            include_str!(concat!("../i18n/en/", $name, ".ftl")),
        )
    };
}

/// The areas, each a file in every language's folder (a test holds the folders to this list).
const AREAS: [(&str, &str, &str); 20] = [
    area!("conflict"),
    area!("drop"),
    area!("errors"),
    area!("format"),
    area!("history"),
    area!("jobs"),
    area!("links"),
    area!("names"),
    area!("notices"),
    area!("pick"),
    area!("places"),
    area!("preview"),
    area!("properties"),
    area!("removal"),
    area!("screen"),
    area!("shell"),
    area!("startup"),
    area!("status"),
    area!("trash"),
    area!("ui"),
];

type Bundle = FluentBundle<FluentResource>;

/// The messages of one language, with English behind them for what they lack.
pub struct Localizer {
    lang: Lang,
    /// The language's own messages, then English's (none for English itself).
    bundles: Vec<Bundle>,
}

impl fmt::Debug for Localizer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Localizer")
            .field("lang", &self.lang)
            .finish_non_exhaustive()
    }
}

impl Localizer {
    /// The messages of `lang`, parsed from the files the build embedded.
    pub fn new(lang: Lang) -> Localizer {
        let fallback = (lang != Lang::En).then(|| bundle(Lang::En, Lang::En.sources()));
        Localizer::of(lang, bundle(lang, lang.sources()), fallback)
    }

    fn of(lang: Lang, own: Bundle, fallback: Option<Bundle>) -> Localizer {
        Localizer {
            lang,
            bundles: std::iter::once(own).chain(fallback).collect(),
        }
    }

    /// The language these messages are in.
    pub fn lang(&self) -> Lang {
        self.lang
    }

    /// The message `key` with `args`: in this language, else in English, else the key itself.
    /// A message whose argument is missing shows its name in braces where the value would be.
    /// Never panics; a debug build notes on stderr what was missing.
    pub fn text(&self, key: &str, args: &[Arg<'_>]) -> String {
        let Some(Formatted {
            text,
            errors,
            fallback,
        }) = self.format(key, args)
        else {
            note(format_args!("no message {key}"));
            return key.to_owned();
        };
        if fallback {
            note(format_args!(
                "{key} is not in {}: shown in English",
                self.lang.code()
            ));
        }
        if !errors.is_empty() {
            note(format_args!("{key}: {errors:?}"));
        }
        text
    }

    /// The message `key` with `args` from the first bundle that has it; `None` when none has.
    fn format(&self, key: &str, args: &[Arg<'_>]) -> Option<Formatted> {
        let pattern = self
            .bundles
            .iter()
            .enumerate()
            .find_map(|(at, bundle)| Some((at, bundle, bundle.get_message(key)?.value()?)));
        let (at, bundle, pattern) = pattern?;
        let mut all = FluentArgs::with_capacity(args.len() + 1);
        all.set(OS_ARG, os_arg(Os::CURRENT));
        for (name, value) in args {
            all.set(*name, value.clone());
        }
        let mut errors = Vec::new();
        let text = bundle.format_pattern(pattern, Some(&all), &mut errors);
        Some(Formatted {
            text: text.into_owned(),
            errors,
            fallback: at > 0,
        })
    }
}

/// A message as [`Localizer::format`] made it.
struct Formatted {
    text: String,
    /// What went wrong in its formatting (a missing argument, an unknown selector).
    errors: Vec<FluentError>,
    /// Whether it came from English, the language lacking it.
    fallback: bool,
}

/// A bundle of `lang` with the files `sources`. A file that does not parse whole, or a message
/// that two files have, is noted (the tests keep the files free of both) and the rest is kept.
fn bundle(lang: Lang, sources: impl Iterator<Item = (&'static str, &'static str)>) -> Bundle {
    let id: LanguageIdentifier = lang.code().parse().unwrap_or_default();
    let mut bundle = Bundle::new_concurrent(vec![id]);
    // Without this, every argument is wrapped in U+2068 and U+2069 (bidi isolation), which
    // would change every Korean sentence with a name or a number in it.
    bundle.set_use_isolating(false);
    for (area, source) in sources {
        let resource =
            FluentResource::try_new(source.to_owned()).unwrap_or_else(|(resource, errors)| {
                note(format_args!("{}/{area}.ftl: {errors:?}", lang.code()));
                resource
            });
        if let Err(errors) = bundle.add_resource(resource) {
            note(format_args!("{}/{area}.ftl: {errors:?}", lang.code()));
        }
    }
    bundle
}

/// Tells the developer, in a debug build, of a message that is missing or did not format.
#[cfg(debug_assertions)]
#[expect(
    clippy::print_stderr,
    reason = "a debug build tells the developer of words that are missing"
)]
fn note(what: fmt::Arguments<'_>) {
    eprintln!("i18n: {what}");
}

/// A release build shows the fallback and says nothing.
#[cfg(not(debug_assertions))]
fn note(_what: fmt::Arguments<'_>) {}

/// The language [`init`] chose.
static CHOSEN: OnceLock<Lang> = OnceLock::new();

/// Sets the language of the run. The first call counts; a later one is ignored, since the
/// words already shown stay as they are (a change takes a restart).
pub fn init(lang: Lang) {
    let _ = CHOSEN.set(lang);
}

/// The language of the run: the one [`init`] set, Korean before it.
pub fn lang() -> Lang {
    CHOSEN.get().copied().unwrap_or_default()
}

/// The messages of `lang`, parsed on the first call.
fn localizer(lang: Lang) -> &'static Localizer {
    static KO: OnceLock<Localizer> = OnceLock::new();
    static EN: OnceLock<Localizer> = OnceLock::new();
    match lang {
        Lang::Ko => KO.get_or_init(|| Localizer::new(Lang::Ko)),
        Lang::En => EN.get_or_init(|| Localizer::new(Lang::En)),
    }
}

/// The message `key` with `args`, in the language of the run ([`Localizer::text`]).
pub fn t(key: &str, args: &[Arg<'_>]) -> String {
    localizer(lang()).text(key, args)
}

/// The message `key`, which has no arguments, in the language of the run, for the functions of
/// the words that return `&'static str`. Each message is made once a language and kept for the
/// rest of the run: at most one copy of each message of each language.
pub fn t_static(key: &'static str) -> &'static str {
    static MADE: LazyLock<Mutex<HashMap<(Lang, &'static str), &'static str>>> =
        LazyLock::new(Mutex::default);
    let lang = lang();
    let mut made = MADE.lock().unwrap_or_else(PoisonError::into_inner);
    made.entry((lang, key))
        .or_insert_with(|| Box::leak(localizer(lang).text(key, &[]).into_boxed_str()))
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use fluent_syntax::ast;

    use super::*;

    /// The messages of `lang`, each with the names of the arguments it uses, from its files.
    fn messages(lang: Lang) -> BTreeMap<String, BTreeSet<String>> {
        messages_in(lang, |_| true)
    }

    /// The messages of `lang` in the files of the areas `wanted` takes, as [`messages`].
    fn messages_in(
        lang: Lang,
        wanted: impl Fn(&str) -> bool,
    ) -> BTreeMap<String, BTreeSet<String>> {
        let mut all = BTreeMap::new();
        for (area, source) in lang.sources().filter(|(area, _)| wanted(area)) {
            let resource = match FluentResource::try_new(source.to_owned()) {
                Ok(resource) => resource,
                Err((_, errors)) => panic!("{}/{area}.ftl: {errors:?}", lang.code()),
            };
            for entry in resource.entries() {
                match entry {
                    ast::Entry::Message(message) => {
                        let mut names = BTreeSet::new();
                        if let Some(pattern) = &message.value {
                            pattern_names(pattern, &mut names);
                        }
                        for attribute in &message.attributes {
                            pattern_names(&attribute.value, &mut names);
                        }
                        let key = message.id.name.to_owned();
                        assert!(
                            all.insert(key.clone(), names).is_none(),
                            "{}: {key} twice",
                            lang.code()
                        );
                    }
                    ast::Entry::Term(term) => panic!(
                        "{}/{area}.ftl: the term -{} (terms do not see `$os`; write the words in the message)",
                        lang.code(),
                        term.id.name
                    ),
                    ast::Entry::Junk { content } => {
                        panic!("{}/{area}.ftl: junk {content:?}", lang.code())
                    }
                    _ => {}
                }
            }
        }
        all
    }

    fn pattern_names(pattern: &ast::Pattern<&str>, names: &mut BTreeSet<String>) {
        for element in &pattern.elements {
            if let ast::PatternElement::Placeable { expression } = element {
                expression_names(expression, names);
            }
        }
    }

    fn expression_names(expression: &ast::Expression<&str>, names: &mut BTreeSet<String>) {
        match expression {
            ast::Expression::Inline(inline) => inline_names(inline, names),
            ast::Expression::Select { selector, variants } => {
                inline_names(selector, names);
                for variant in variants {
                    pattern_names(&variant.value, names);
                }
            }
        }
    }

    fn inline_names(inline: &ast::InlineExpression<&str>, names: &mut BTreeSet<String>) {
        match inline {
            ast::InlineExpression::VariableReference { id } => {
                names.insert(id.name.to_owned());
            }
            ast::InlineExpression::FunctionReference { arguments, .. } => {
                for argument in &arguments.positional {
                    inline_names(argument, names);
                }
                for argument in &arguments.named {
                    inline_names(&argument.value, names);
                }
            }
            ast::InlineExpression::Placeable { expression } => expression_names(expression, names),
            ast::InlineExpression::MessageReference { id, .. } => {
                panic!(
                    "a reference to the message {} (write the words out)",
                    id.name
                )
            }
            _ => {}
        }
    }

    #[test]
    fn every_file_parses_and_both_languages_have_the_same_keys() {
        let ko = messages(Lang::Ko);
        let en = messages(Lang::En);
        let ko_keys: BTreeSet<&String> = ko.keys().collect();
        let en_keys: BTreeSet<&String> = en.keys().collect();
        assert_eq!(
            ko_keys.difference(&en_keys).collect::<Vec<_>>(),
            Vec::<&&String>::new(),
            "only in Korean"
        );
        assert_eq!(
            en_keys.difference(&ko_keys).collect::<Vec<_>>(),
            Vec::<&&String>::new(),
            "only in English"
        );
        assert!(ko.len() > 100, "{} messages", ko.len());
        // Every key is `area-meaning`, in lowercase words.
        for key in ko.keys() {
            assert!(
                key.contains('-')
                    && key
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
                "{key}"
            );
        }
        // The bundles take every file without a message twice.
        for lang in Lang::ALL {
            let id: LanguageIdentifier = lang.code().parse().expect("a language");
            let mut bundle = Bundle::new_concurrent(vec![id]);
            for (area, source) in lang.sources() {
                let resource = FluentResource::try_new(source.to_owned()).expect("parses");
                assert!(
                    bundle.add_resource(resource).is_ok(),
                    "{}/{area}.ftl repeats a key",
                    lang.code()
                );
            }
        }
    }

    #[test]
    fn both_languages_use_the_same_arguments_but_the_korean_ones() {
        let ko = messages(Lang::Ko);
        let en = messages(Lang::En);
        for (key, ko_names) in &ko {
            let en_names = &en[key];
            // `$os` is given to every message, which may use it or not.
            let shared = |names: &BTreeSet<String>| -> BTreeSet<String> {
                names
                    .iter()
                    .filter(|name| *name != OS_ARG && !name.starts_with("ko-"))
                    .cloned()
                    .collect()
            };
            assert_eq!(shared(ko_names), shared(en_names), "{key}");
            assert!(
                en_names.iter().all(|name| !name.starts_with("ko-")),
                "{key}: English uses a Korean argument: {en_names:?}"
            );
        }
    }

    #[test]
    fn every_message_formats_in_both_languages_without_an_error_or_isolation_marks() {
        for lang in Lang::ALL {
            let localizer = Localizer::new(lang);
            for (key, names) in messages(lang) {
                // A name, then a number, for each argument; every system.
                for os in [Os::Windows, Os::MacOs, Os::Other] {
                    for dummy in [FluentValue::from("x"), FluentValue::from(1), number(2)] {
                        let mut args: Vec<Arg<'_>> = names
                            .iter()
                            .map(|name| (name.as_str(), dummy.clone()))
                            .collect();
                        args.push((OS_ARG, os_arg(os)));
                        let Formatted {
                            text,
                            errors,
                            fallback,
                        } = localizer.format(&key, &args).expect("a message");
                        assert!(!fallback, "{}: {key} is English", lang.code());
                        assert!(errors.is_empty(), "{}: {key}: {errors:?}", lang.code());
                        assert!(!text.is_empty(), "{}: {key}", lang.code());
                        assert!(
                            !text.contains(['\u{2068}', '\u{2069}']),
                            "{}: {key}: {text:?}",
                            lang.code()
                        );
                        assert_eq!(text.trim(), text, "{}: {key}", lang.code());
                    }
                }
            }
        }
    }

    #[test]
    fn every_area_file_is_listed() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("i18n");
        for lang in Lang::ALL {
            let mut files: Vec<String> = std::fs::read_dir(root.join(lang.code()))
                .expect("the language's folder")
                .map(|entry| {
                    entry
                        .expect("an entry")
                        .file_name()
                        .to_string_lossy()
                        .into_owned()
                })
                .collect();
            files.sort();
            let listed: Vec<String> = AREAS
                .iter()
                .map(|(area, _, _)| format!("{area}.ftl"))
                .collect();
            assert_eq!(files, listed, "{}", lang.code());
        }
    }

    #[test]
    fn the_language_follows_the_setting_then_the_system() {
        use LanguageChoice as C;
        for (locale, lang) in [
            (Some("ko-KR"), Lang::Ko),
            (Some("ko"), Lang::Ko),
            (Some("ko_KR.UTF-8"), Lang::Ko),
            (Some("KO-kr"), Lang::Ko),
            (Some("ko.UTF-8"), Lang::Ko),
            (Some("en-US"), Lang::En),
            (Some("ja-JP"), Lang::En),
            (Some("kok-IN"), Lang::En),
            (Some("C"), Lang::En),
            (Some(""), Lang::En),
            (None, Lang::En),
        ] {
            assert_eq!(pick(C::System, locale), lang, "{locale:?}");
            // A language the settings name is the one, whatever the system says.
            assert_eq!(pick(C::Ko, locale), Lang::Ko, "{locale:?}");
            assert_eq!(pick(C::En, locale), Lang::En, "{locale:?}");
        }
        assert_eq!(LanguageChoice::default(), C::System);
    }

    /// A localizer of `lang` with `own` as its file and `fallback` as English's.
    fn made(lang: Lang, own: &'static str, fallback: Option<&'static str>) -> Localizer {
        Localizer::of(
            lang,
            bundle(lang, [("test", own)].into_iter()),
            fallback.map(|source| bundle(Lang::En, [("test", source)].into_iter())),
        )
    }

    #[test]
    fn a_missing_message_shows_in_english_then_as_its_key() {
        let localizer = made(
            Lang::Ko,
            "both = 둘 다\n",
            Some("both = Both\nenglish-only = Only { $what }\n"),
        );
        assert_eq!(localizer.text("both", &[]), "둘 다");
        assert_eq!(
            localizer.text("english-only", &[("what", "English".into())]),
            "Only English"
        );
        assert_eq!(localizer.text("nowhere-at-all", &[]), "nowhere-at-all");
        // What a debug build notes: the English that stood in for a Korean message.
        let fallback = |key: &str| localizer.format(key, &[]).map(|made| made.fallback);
        assert_eq!(fallback("both"), Some(false));
        assert_eq!(fallback("english-only"), Some(true));
        assert_eq!(fallback("nowhere-at-all"), None);
        // A missing argument shows its name, and nothing panics.
        assert_eq!(localizer.text("english-only", &[]), "Only {$what}");
        // English has nothing behind it.
        let english = made(Lang::En, "both = Both\n", None);
        assert_eq!(english.text("english-only", &[]), "english-only");
    }

    #[test]
    fn numbers_show_as_rust_shows_them() {
        let localizer = made(
            Lang::En,
            "n = { $n }\nitems = { $n ->\n    [one] 1 item\n   *[other] { $n } items\n}\n",
            None,
        );
        for n in [0, 1, 12, 1024, 1 << 40, EXACT, EXACT + 1, u64::MAX] {
            assert_eq!(localizer.text("n", &[("n", number(n))]), n.to_string());
        }
        assert_eq!(localizer.text("items", &[("n", count(1))]), "1 item");
        assert_eq!(localizer.text("items", &[("n", count(0))]), "0 items");
        assert_eq!(localizer.text("items", &[("n", count(3))]), "3 items");
        // Past what a Fluent number holds exactly, the text is whole and the plural `other`.
        assert_eq!(
            localizer.text("items", &[("n", number(u64::MAX))]),
            format!("{} items", u64::MAX)
        );
    }

    #[test]
    fn the_run_speaks_korean_until_it_is_told_otherwise() {
        // No test calls `init`: the words of every test are Korean.
        assert_eq!(lang(), Lang::Ko);
        assert_eq!(t("place-trash", &[]), "휴지통");
        assert_eq!(t_static("place-trash"), "휴지통");
        // Made once, then the same text.
        assert!(std::ptr::eq(
            t_static("place-trash"),
            t_static("place-trash")
        ));
    }

    /// Test support that is compiled into the library (`rf_vfs::testkit`, which the tests of
    /// other crates call), and its items whose Hangul literals are allowed for good: the names
    /// of the files its checks make, which a provider must store and list whole in any script,
    /// never words of the app. Each item must still hold one, or its allowance goes.
    const FIXTURES: [(&str, &[&str]); 1] = [(
        "crates/rf-vfs/src/testkit.rs",
        &["UNICODE_NAMES", "check_long_paths"],
    )];

    /// Whether `c` is Hangul: a syllable or a jamo.
    fn is_hangul(c: char) -> bool {
        matches!(c,
            '\u{1100}'..='\u{11FF}'
            | '\u{3130}'..='\u{318F}'
            | '\u{A960}'..='\u{A97F}'
            | '\u{AC00}'..='\u{D7A3}'
            | '\u{D7B0}'..='\u{D7FF}')
    }

    /// How a literal is written.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Written {
        /// `"…"` or `b"…"`, whose escapes stand for characters.
        Plain,
        /// `r"…"`, `r#"…"#`: as it is.
        Raw,
        /// `'…'`, one character.
        Char,
    }

    /// A string or character literal: where it starts and where it ends (after its closing
    /// quote), and its text between the quotes, as written.
    struct Literal {
        at: usize,
        end: usize,
        text: String,
        written: Written,
    }

    impl Literal {
        /// The text with each `\u{…}` escape turned into the character it stands for (a raw
        /// string has none); the other escapes as written.
        fn decoded(&self) -> String {
            if self.written == Written::Raw {
                return self.text.clone();
            }
            let mut out = String::new();
            let mut chars = self.text.chars();
            while let Some(c) = chars.next() {
                if c != '\\' {
                    out.push(c);
                    continue;
                }
                match chars.next() {
                    Some('u') => {
                        let code: String = chars.by_ref().take_while(|&c| c != '}').collect();
                        let decoded = code
                            .strip_prefix('{')
                            .and_then(|hex| u32::from_str_radix(&hex.replace('_', ""), 16).ok())
                            .and_then(char::from_u32);
                        if let Some(decoded) = decoded {
                            out.push(decoded);
                        } else {
                            out.push_str("\\u");
                            out.push_str(&code);
                            out.push('}');
                        }
                    }
                    Some(other) => {
                        out.push('\\');
                        out.push(other);
                    }
                    None => out.push('\\'),
                }
            }
            out
        }
    }

    /// A Rust file read for its literals: its text with the comments and the literals blanked
    /// out, and the literals.
    struct Scanned {
        masked: Vec<char>,
        literals: Vec<Literal>,
    }

    /// The literal of `chars` that starts at `at` and ends at `end`, its text from `from` to `to`.
    fn literal(
        chars: &[char],
        at: usize,
        (from, to): (usize, usize),
        end: usize,
        written: Written,
    ) -> Literal {
        Literal {
            at,
            end,
            text: chars[from.min(chars.len())..to.min(chars.len())]
                .iter()
                .collect(),
            written,
        }
    }

    /// `chars` with its comments and its string and character literals blanked (the line breaks
    /// kept), and the literals as written, escapes and all.
    fn scan(chars: &[char]) -> Scanned {
        let mut masked = chars.to_vec();
        let mut literals = Vec::new();
        let blank = |masked: &mut Vec<char>, from: usize, to: usize| {
            for c in &mut masked[from..to.min(chars.len())] {
                if *c != '\n' {
                    *c = ' ';
                }
            }
        };
        let at = |i: usize| chars.get(i).copied();
        let mut i = 0;
        while i < chars.len() {
            let c = chars[i];
            let in_word = i > 0 && (chars[i - 1].is_alphanumeric() || chars[i - 1] == '_');
            if c == '/' && at(i + 1) == Some('/') {
                let end = chars[i..]
                    .iter()
                    .position(|&c| c == '\n')
                    .map_or(chars.len(), |n| i + n);
                blank(&mut masked, i, end);
                i = end;
            } else if c == '/' && at(i + 1) == Some('*') {
                let start = i;
                let mut depth = 0;
                while i < chars.len() {
                    if chars[i] == '/' && at(i + 1) == Some('*') {
                        depth += 1;
                        i += 2;
                    } else if chars[i] == '*' && at(i + 1) == Some('/') {
                        depth -= 1;
                        i += 2;
                        if depth == 0 {
                            break;
                        }
                    } else {
                        i += 1;
                    }
                }
                blank(&mut masked, start, i);
            } else if !in_word && (c == 'r' || (c == 'b' && at(i + 1) == Some('r'))) && {
                let mut j = i + if c == 'b' { 2 } else { 1 };
                while at(j) == Some('#') {
                    j += 1;
                }
                at(j) == Some('"')
            } {
                let mut j = i + if c == 'b' { 2 } else { 1 };
                let mut hashes = 0;
                while at(j) == Some('#') {
                    hashes += 1;
                    j += 1;
                }
                let body = j + 1;
                let mut end = body;
                while end < chars.len()
                    && !(chars[end] == '"' && (1..=hashes).all(|h| at(end + h) == Some('#')))
                {
                    end += 1;
                }
                let after = end + 1 + hashes;
                literals.push(literal(chars, i, (body, end), after, Written::Raw));
                blank(&mut masked, i, after);
                i = after;
            } else if c == '"' {
                let mut end = i + 1;
                while end < chars.len() && chars[end] != '"' {
                    end += if chars[end] == '\\' { 2 } else { 1 };
                }
                literals.push(literal(chars, i, (i + 1, end), end + 1, Written::Plain));
                blank(&mut masked, i, end + 1);
                i = end + 1;
            } else if c == '\'' && (at(i + 1) == Some('\\') || at(i + 2) == Some('\'')) {
                // A character, not a lifetime.
                let mut end = i + 2;
                while end < chars.len() && chars[end] != '\'' {
                    end += 1;
                }
                literals.push(literal(chars, i, (i + 1, end), end + 1, Written::Char));
                blank(&mut masked, i, end + 1);
                i = end + 1;
            } else {
                i += 1;
            }
        }
        Scanned { masked, literals }
    }

    /// Whether `literal` is a character that bounds a range (`'\u{AC00}'..='\u{D7A3}'`): a code
    /// point the code compares with, never a word.
    fn is_range_bound(masked: &[char], literal: &Literal) -> bool {
        if literal.written != Written::Char {
            return false;
        }
        let mut after = literal.end;
        while masked.get(after).is_some_and(|c| c.is_whitespace()) {
            after += 1;
        }
        let mut before = literal.at;
        while before > 0 && masked[before - 1].is_whitespace() {
            before -= 1;
        }
        let ends_with = |tail: &str| {
            let tail: Vec<char> = tail.chars().collect();
            before >= tail.len() && masked[before - tail.len()..before] == tail[..]
        };
        masked.get(after..after + 2) == Some(&['.', '.'][..]) || ends_with("..") || ends_with("..=")
    }

    /// Whether the attribute `#[cfg(…` (to its `]`) keeps its item out of every build but the
    /// tests': `cfg(test)`, or `cfg(all(…))` with `test` among its conditions. `any(test, …)`
    /// and `not(…)` may hold outside the tests, so their items count as the app's.
    fn only_in_tests(attribute: &str) -> bool {
        let Some(condition) = attribute
            .trim()
            .strip_prefix("#[cfg(")
            .and_then(|rest| rest.trim_end().strip_suffix(')'))
            .map(str::trim)
        else {
            return false;
        };
        if condition == "test" {
            return true;
        }
        let Some(all) = condition
            .strip_prefix("all(")
            .and_then(|rest| rest.strip_suffix(')'))
        else {
            return false;
        };
        let mut depth = 0;
        let mut part = String::new();
        let mut parts = Vec::new();
        for c in all.chars() {
            match c {
                '(' => depth += 1,
                ')' => depth -= 1,
                ',' if depth == 0 => {
                    parts.push(std::mem::take(&mut part));
                    continue;
                }
                _ => {}
            }
            part.push(c);
        }
        parts.push(part);
        parts.iter().any(|part| part.trim() == "test")
    }

    /// Where the item that starts at `from` in `masked` ends: at its first `;` or `,` outside
    /// brackets (an item of a list, a field, an arm of a match), at the `}` of its first `{`,
    /// or where the brackets around it close.
    fn item_end(masked: &[char], from: usize) -> usize {
        let mut j = from;
        let mut depth = 0;
        while j < masked.len() {
            match masked[j] {
                '(' | '[' => depth += 1,
                ')' | ']' | '}' | ';' | ',' if depth == 0 => break,
                ')' | ']' => depth -= 1,
                '{' if depth == 0 => {
                    let mut braces = 0;
                    while j < masked.len() {
                        match masked[j] {
                            '{' => braces += 1,
                            '}' => {
                                braces -= 1;
                                if braces == 0 {
                                    break;
                                }
                            }
                            _ => {}
                        }
                        j += 1;
                    }
                    break;
                }
                _ => {}
            }
            j += 1;
        }
        j
    }

    /// The parts of `masked` that only tests compile: each item under a `#[cfg(…)]` that
    /// keeps it to the tests ([`only_in_tests`]), as places from to; and the names of such items
    /// that are modules in files of their own (`#[cfg(test)] mod tests;`).
    fn test_items(masked: &[char]) -> (Vec<(usize, usize)>, Vec<String>) {
        let text: String = masked.iter().collect();
        let mut ranges = Vec::new();
        let mut modules = Vec::new();
        let mut from = 0;
        while let Some(found) = text[from..].find("#[cfg(") {
            let start = from + found;
            let start_char = text[..start].chars().count();
            // The attribute, to its `]`.
            let mut depth = 0;
            let mut i = start_char + 1;
            while i < masked.len() {
                match masked[i] {
                    '[' => depth += 1,
                    ']' => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
                i += 1;
            }
            let attribute: String = masked[start_char..i.min(masked.len())].iter().collect();
            from = start + 1;
            if !only_in_tests(&attribute) {
                continue;
            }
            let j = item_end(masked, i + 1);
            let item: String = masked[(i + 1).min(masked.len())..j.min(masked.len())]
                .iter()
                .collect();
            let item = item.trim();
            if let Some(name) = item
                .strip_prefix("mod ")
                .or_else(|| item.strip_prefix("pub mod "))
                .filter(|name| !name.contains('{'))
            {
                modules.push(name.trim().to_owned());
            }
            ranges.push((start_char, j));
        }
        (ranges, modules)
    }

    /// Where the item `name` of `masked` (a `fn`, a `const` or a `static`) starts and ends.
    fn item_named(masked: &[char], name: &str) -> Option<(usize, usize)> {
        let text: String = masked.iter().collect();
        let at = ["fn ", "const ", "static "].iter().find_map(|kind| {
            let wanted = format!("{kind}{name}");
            text.match_indices(&wanted)
                .find(|(at, _)| {
                    !text[at + wanted.len()..]
                        .starts_with(|c: char| c.is_alphanumeric() || c == '_')
                })
                .map(|(at, _)| at)
        })?;
        let start = text[..at].chars().count();
        Some((start, item_end(masked, start)))
    }

    /// The Hangul literals of the file `scanned` outside the items only tests compile
    /// (`tests`), but the bounds of a range, read with their escapes decoded.
    fn hangul_literals<'a>(scanned: &'a Scanned, tests: &[(usize, usize)]) -> Vec<&'a Literal> {
        scanned
            .literals
            .iter()
            .filter(|literal| {
                !tests
                    .iter()
                    .any(|&(from, to)| (from..to).contains(&literal.at))
            })
            .filter(|literal| !is_range_bound(&scanned.masked, literal))
            .filter(|literal| literal.decoded().chars().any(is_hangul))
            .collect()
    }

    /// The Rust files under `dir`, but those of test, bench and example folders and of the
    /// build's output.
    fn rust_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries {
            let path = entry.expect("an entry").path();
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned());
            if path.is_dir() {
                if !matches!(
                    name.as_deref(),
                    Some("target" | "tests" | "benches" | "examples")
                ) {
                    rust_files(&path, out);
                }
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                out.push(path);
            }
        }
    }

    /// The rule of I18N-02 after Task 9: the words of the app are in the Fluent files, so no
    /// Rust code outside tests holds a Hangul string or character literal (escapes decoded), but
    /// in what [`FIXTURES`] allows. A character that bounds a range is a code point, not a word.
    #[test]
    fn no_hangul_string_literal_outside_tests() {
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let workspace = manifest
            .parent()
            .and_then(std::path::Path::parent)
            .expect("the workspace");
        let mut files = Vec::new();
        rust_files(&workspace.join("crates"), &mut files);
        rust_files(&workspace.join("apps"), &mut files);
        let relative = |path: &std::path::Path| {
            path.strip_prefix(workspace)
                .expect("in the workspace")
                .components()
                .map(|part| part.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/")
        };
        let names: Vec<String> = files.iter().map(|path| relative(path)).collect();
        for reached in [
            "crates/rf-ui/src/words.rs",
            "crates/rf-ui/src/format.rs",
            "crates/rf-platform/src/links/mod.rs",
        ] {
            assert!(
                names.iter().any(|name| name == reached),
                "{reached}: {names:?}"
            );
        }
        let mut test_files = Vec::new();
        let mut found: Vec<(String, usize, String)> = Vec::new();
        let mut needed: Vec<(String, &str)> = Vec::new();
        for (path, name) in files.iter().zip(&names) {
            let source = std::fs::read_to_string(path).expect("a Rust file");
            let chars: Vec<char> = source.chars().collect();
            let scanned = scan(&chars);
            let (tests, modules) = test_items(&scanned.masked);
            // A test-only module in a file of its own: `dir/name.rs` or `dir/name/mod.rs`.
            let stem = path
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned());
            let dir = match stem.as_deref() {
                Some("mod" | "lib" | "main") => path.parent().map(std::path::Path::to_path_buf),
                Some(stem) => path.parent().map(|parent| parent.join(stem)),
                None => None,
            };
            if let Some(dir) = dir {
                for module in modules {
                    test_files.push(relative(&dir.join(format!("{module}.rs"))));
                    test_files.push(relative(&dir.join(&module).join("mod.rs")));
                }
            }
            let allowed: Vec<(&str, (usize, usize))> = FIXTURES
                .iter()
                .filter(|(file, _)| file == name)
                .flat_map(|(_, items)| items.iter())
                .map(|&item| {
                    let span = item_named(&scanned.masked, item)
                        .unwrap_or_else(|| panic!("{name} has no item {item}"));
                    (item, span)
                })
                .collect();
            for literal in hangul_literals(&scanned, &tests) {
                let fixture = allowed
                    .iter()
                    .find(|(_, (from, to))| (*from..*to).contains(&literal.at));
                if let Some((item, _)) = fixture {
                    needed.push((name.clone(), item));
                } else {
                    let line = chars[..literal.at].iter().filter(|&&c| c == '\n').count() + 1;
                    found.push((name.clone(), line, literal.text.clone()));
                }
            }
        }
        found.retain(|(name, _, _)| !test_files.contains(name));
        assert!(
            found.is_empty(),
            "Hangul literals outside tests (move the words to crates/rf-ui/i18n): {found:#?}"
        );
        // An allowance goes once its item holds no Hangul literal.
        for (file, items) in FIXTURES {
            for &item in items {
                assert!(
                    needed.contains(&(file.to_owned(), item)),
                    "{file}: {item} needs no allowance any more: take it out of FIXTURES"
                );
            }
        }
    }

    #[test]
    fn the_scan_finds_literals_and_skips_comments_and_tests() {
        let source = concat!(
            "// ‘주석’ \"아님\"\n",
            "fn a() { let x = \"보임\"; let c = '가'; let r = r#\"원\"#; let n = \"글\"; }\n",
            "/* \"블록\" */\n",
            "fn b<'a>(_: &'a str) {}\n",
            "#[cfg(all(test, unix))]\nmod t { fn c() { let _ = \"시험\"; } }\n",
            "#[cfg(not(test))]\nfn d() { let _ = \"보임2\"; }\n",
            // Conditions that hold outside the tests too.
            "#[cfg(any(test, unix))]\nfn e() { let _ = \"보임3\"; }\n",
            "#[cfg(not(any(test, debug_assertions)))]\nfn f() { let _ = \"보임4\"; }\n",
            // An attribute on an arm of a match keeps that arm alone.
            "fn g(x: u8) -> &'static str {\n",
            "    match x { #[cfg(test)] 0 => \"시험2\", _ => { \"보임5\" } }\n",
            "}\n",
            // Escapes are read; the bounds of a range are code points.
            "fn h() { let _ = \"\\u{d734}\\u{c9c0}\"; let _ = r\"\\u{d734}\"; ",
            "let _ = ('\\u{AC00}'..='\\u{D7A3}', '\\u{AC00}'); let _ = \"\\\\u{AC00}\"; }\n",
            "#[cfg(test)]\nmod tests;\n",
        );
        let chars: Vec<char> = source.chars().collect();
        let scanned = scan(&chars);
        let (tests, modules) = test_items(&scanned.masked);
        let shown: Vec<&str> = hangul_literals(&scanned, &tests)
            .into_iter()
            .map(|literal| literal.text.as_str())
            .collect();
        assert_eq!(
            shown,
            [
                "보임",
                "가",
                "원",
                "글",
                "보임2",
                "보임3",
                "보임4",
                "보임5",
                "\\u{d734}\\u{c9c0}",
                "\\u{AC00}"
            ]
        );
        assert_eq!(modules, ["tests"]);
        // The attribute as `test_items` reads it, to its `]`.
        assert!(only_in_tests("#[cfg(test)"));
        assert!(only_in_tests("#[cfg(all(unix, test))"));
        assert!(!only_in_tests("#[cfg(all(unix, any(test, windows)))"));
        assert!(!only_in_tests("#[cfg(unix)"));
    }

    /// The calls in `masked` whose parentheses hold the key of a message: the name before each
    /// `(` and where it opens and closes.
    fn calls(masked: &[char]) -> Vec<(String, usize, usize)> {
        let mut made = Vec::new();
        let mut open: Vec<(String, usize)> = Vec::new();
        for (i, &c) in masked.iter().enumerate() {
            match c {
                '(' => {
                    let mut start = i;
                    while start > 0
                        && (masked[start - 1].is_alphanumeric() || masked[start - 1] == '_')
                    {
                        start -= 1;
                    }
                    open.push((masked[start..i].iter().collect(), i));
                }
                ')' => {
                    if let Some((name, at)) = open.pop() {
                        made.push((name, at, i));
                    }
                }
                _ => {}
            }
        }
        made
    }

    /// The calls that make a message with no argument but `$os`: [`t_static`], and `format.rs`'s
    /// closure `named`, which asks it.
    const STATIC_CALLS: [&str; 2] = ["t_static", "named"];

    /// The calls that make a message with arguments: [`t`], and the words' helpers that call it
    /// with a key they are given.
    const CALLS: [&str; 6] = ["t", "says", "with_reason", "of_app", "sized", "of_kind"];

    /// Whether `literal` is written as a key is: lowercase words joined by `-`. A `ko-` name is
    /// a Korean argument's, not a key.
    fn is_key(literal: &str) -> bool {
        literal.contains('-')
            && !literal.starts_with('-')
            && !literal.starts_with("ko-")
            && literal
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    }

    /// Every key the words ask for is in both languages; and each one they ask [`t_static`] for
    /// (a key whose nearest call is it) takes no argument but `$os` in either, so that no message
    /// is kept for good with `{$name}` in it.
    #[test]
    fn every_key_the_moved_code_asks_for_is_in_both_languages() {
        let ko = messages(Lang::Ko);
        let en = messages(Lang::En);
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut asked = 0;
        let mut kept = 0;
        for file in ["words.rs", "format.rs"] {
            let source = std::fs::read_to_string(src.join(file)).expect("a Rust file");
            // The code outside the tests.
            let moved = source.split("#[cfg(test)]").next().unwrap_or_default();
            let chars: Vec<char> = moved.chars().collect();
            let scanned = scan(&chars);
            let calls = calls(&scanned.masked);
            let line = |at: usize| chars[..at].iter().filter(|&&c| c == '\n').count() + 1;
            for literal in scanned.literals.iter().filter(|l| is_key(&l.text)) {
                let key = &literal.text;
                asked += 1;
                assert!(ko.contains_key(key), "{file}: {key} is not in ko");
                assert!(en.contains_key(key), "{file}: {key} is not in en");
                let nearest = calls
                    .iter()
                    .filter(|(name, from, to)| {
                        (*from..*to).contains(&literal.at)
                            && (STATIC_CALLS.contains(&name.as_str())
                                || CALLS.contains(&name.as_str()))
                    })
                    .max_by_key(|(_, from, _)| *from);
                if nearest.is_some_and(|(name, _, _)| STATIC_CALLS.contains(&name.as_str())) {
                    kept += 1;
                    for (lang, all) in [("ko", &ko), ("en", &en)] {
                        let names = &all[key];
                        assert!(
                            names.iter().all(|name| name == OS_ARG),
                            "{file}:{}: {key} is kept by t_static but takes {names:?} in {lang}",
                            line(literal.at)
                        );
                    }
                }
            }
            // `t_static` is given its key where this test sees it: a key, or a `match` or an
            // `if` of keys, never a name that stands for one (but in the closure `named`).
            for (name, from, to) in &calls {
                let start = scanned.masked[..*from]
                    .iter()
                    .rposition(|&c| c == '\n')
                    .map_or(0, |at| at + 1);
                let before: String = scanned.masked[start..*from].iter().collect();
                let sees_a_key = scanned
                    .literals
                    .iter()
                    .any(|l| (*from..*to).contains(&l.at) && is_key(&l.text));
                assert!(
                    name != "t_static"
                        || sees_a_key
                        || before.trim() == "let named = |key| t_static",
                    "{file}:{}: t_static is given a key this test cannot see",
                    line(*from)
                );
            }
        }
        assert!(asked > 400, "{asked} keys");
        assert!(kept > 200, "{kept} keys kept by t_static");
    }

    #[test]
    fn english_says_trash_or_recycle_bin_as_the_system_does() {
        let en = Localizer::new(Lang::En);
        let ko = Localizer::new(Lang::Ko);
        let on = |os| [(OS_ARG, os_arg(os))];
        assert_eq!(en.text("place-trash", &on(Os::Windows)), "Recycle Bin");
        assert_eq!(en.text("place-trash", &on(Os::MacOs)), "Trash");
        assert_eq!(en.text("place-trash", &on(Os::Other)), "Trash");
        for os in [Os::Windows, Os::MacOs, Os::Other] {
            assert_eq!(ko.text("place-trash", &on(os)), "휴지통");
        }
    }

    #[test]
    fn english_counts_names_the_work_and_the_system() {
        let en = Localizer::new(Lang::En);
        let ko = Localizer::new(Lang::Ko);
        let n = |key: &str, value: usize| en.text(key, &[("count", count(value))]);
        // Plurals.
        assert_eq!(n("status-items", 1), "1 item");
        assert_eq!(n("status-items", 9), "9 items");
        assert_eq!(n("status-plus-folders", 1), "+1 folder");
        assert_eq!(n("pattern-count", 0), "0 matches");
        assert_eq!(en.text("size-bytes", &[("bytes", number(1))]), "1 byte");
        assert_eq!(
            en.text("size-bytes", &[("bytes", number(512))]),
            "512 bytes"
        );
        assert_eq!(
            en.text(
                "job-progress-items",
                &[("done", number(1)), ("total", number(1))]
            ),
            "1 / 1 item"
        );
        assert_eq!(
            en.text(
                "duration-minutes-seconds",
                &[("minutes", number(1)), ("seconds", number(20))]
            ),
            "1 minute 20 seconds"
        );
        // A job's sentence, whole in English, and the trash as the system names it.
        let work =
            |work: &'static str, os: Os| [("work", FluentValue::from(work)), (OS_ARG, os_arg(os))];
        assert_eq!(en.text("job-done", &work("copy", Os::Other)), "Copied");
        assert_eq!(
            en.text("job-done", &work("trash", Os::Windows)),
            "Moved to the Recycle Bin"
        );
        assert_eq!(
            en.text("job-done", &work("trash", Os::MacOs)),
            "Moved to the Trash"
        );
        assert_eq!(
            en.text("job-cancelled", &work("move", Os::Other)),
            "Moving cancelled"
        );
        assert_eq!(
            en.text(
                "job-notice-done-with-errors",
                &[("work", "copy".into()), ("failed", count(1))]
            ),
            "Copying finished, but 1 item couldn't be processed"
        );
        assert_eq!(
            en.text(
                "job-report-done",
                &[("work", "link".into()), ("count", count(2))]
            ),
            "2 links created"
        );
        // A work the messages do not know reads as the general one, in both languages.
        assert_eq!(en.text("job-done", &work("compress", Os::Other)), "Done");
        assert_eq!(
            ko.text("job-done", &work("compress", Os::Other)),
            "작업을 마쳤습니다"
        );
        assert_eq!(
            ko.text("job-done", &work("trash", Os::Windows)),
            "휴지통으로 보냈습니다"
        );
    }

    #[test]
    fn english_says_the_notices_refusals_and_properties_whole() {
        let en = Localizer::new(Lang::En);
        let on = |os| (OS_ARG, os_arg(os));
        let work = |work: &'static str| ("work", FluentValue::from(work));
        // Plurals, and the trash as the system names it.
        assert_eq!(
            en.text("job-items-done", &[work("copy"), ("count", count(1))]),
            "1 item copied"
        );
        assert_eq!(
            en.text(
                "job-items-done",
                &[work("trash"), ("count", count(3)), on(Os::Windows)]
            ),
            "3 items moved to the Recycle Bin"
        );
        assert_eq!(
            en.text(
                "job-items-partly-done",
                &[
                    work("drag"),
                    ("count", number(3)),
                    ("done", number(1)),
                    ("skipped", number(2))
                ]
            ),
            "1 of 3 items moved · 2 skipped"
        );
        assert_eq!(
            en.text("trash-refused", &[("count", count(1)), on(Os::MacOs)]),
            "The Trash didn't take 1 item.\nPermanently delete it? This can't be undone."
        );
        assert_eq!(
            en.text("copies-delete", &[("count", count(2))]),
            "Permanently delete 2 copies? This can't be undone."
        );
        assert_eq!(
            en.text(
                "job-stopped-note-left",
                &[work("copy"), ("count", number(1))]
            ),
            "Copying stopped at the items below, and the 1 item after them was not processed. \
             To finish, start copying again."
        );
        assert_eq!(
            en.text("trash-empty-title", &[on(Os::Windows)]),
            "Empty Recycle Bin"
        );
        assert_eq!(
            en.text("trash-empty-title", &[on(Os::Other)]),
            "Empty Trash"
        );
        // What can't be done in a place of the shell, and why a drop or a folder is refused.
        let blocked = |place: &'static str, what: &'static str, os| {
            en.text(
                "shell-blocked",
                &[("place", place.into()), ("what", what.into()), on(os)],
            )
        };
        assert_eq!(
            blocked("trash", "new_folder", Os::Windows),
            "Can't create a folder in the Recycle Bin. Items there can be restored or deleted \
             permanently."
        );
        assert_eq!(
            blocked("computer", "paste", Os::MacOs),
            "Can't paste in Computer. Open a drive to do it there."
        );
        assert_eq!(
            en.text("drop-refused-same-folder", &[]),
            "Can't drop here: the items are already in this folder"
        );
        assert_eq!(
            en.text("pick-problem-shell-place", &[on(Os::Windows)]),
            "Nothing can be put in the Recycle Bin or This PC"
        );
        assert_eq!(
            en.text(
                "pick-problem-space-at-least",
                &[("need", "3 MB".into()), ("available", "1 MB".into())]
            ),
            "There may not be enough space: at least 3 MB needed, 1 MB free"
        );
        // English names the item, never uses the Korean particle.
        let conflict = |ko_final: &'static str| {
            [
                ("name", FluentValue::from("photos")),
                ("folder", FluentValue::from("C:\\Backup")),
                ("ko-final", FluentValue::from(ko_final)),
            ]
        };
        assert_eq!(
            en.text("conflict-description", &conflict("yes")),
            "“photos” already exists in C:\\Backup."
        );
        let ko = Localizer::new(Lang::Ko);
        assert_eq!(
            ko.text("conflict-description", &conflict("yes")),
            "‘photos’이 C:\\Backup 에 이미 있습니다."
        );
        assert_eq!(
            ko.text("conflict-description", &conflict("no")),
            "‘photos’가 C:\\Backup 에 이미 있습니다."
        );
    }

    #[test]
    fn english_names_the_properties_a_date_and_a_shortcut() {
        let en = Localizer::new(Lang::En);
        let on = |os| (OS_ARG, os_arg(os));
        // The labels of the properties window.
        let label = |key: &str, os| en.text(key, &[on(os)]);
        assert_eq!(label("fact-kind", Os::MacOs), "Kind");
        assert_eq!(label("fact-kind", Os::Windows), "Type");
        for (key, text) in [
            ("fact-place", "Location"),
            ("fact-size", "Size"),
            ("fact-contents", "Contains"),
            ("fact-link-target", "Link Target"),
            ("fact-created", "Created"),
            ("fact-modified", "Modified"),
            ("fact-accessed", "Accessed"),
            ("fact-attributes", "Attributes"),
            ("attribute-read-only", "Read-only"),
            ("attribute-none", "None"),
        ] {
            assert_eq!(label(key, Os::Other), text, "{key}");
        }
        assert_eq!(
            en.text(
                "properties-files-and-folders",
                &[("files", number(1)), ("folders", number(678))]
            ),
            "1 file, 678 folders"
        );
        // A date before today, and the word of a new shortcut's name.
        assert_eq!(
            en.text(
                "history-time-day",
                &[
                    ("month", number(10)),
                    ("day", number(30)),
                    ("time", "14:32".into())
                ]
            ),
            "Oct 30, 14:32"
        );
        assert_eq!(en.text("link-name-shortcut", &[]), "Shortcut");
        assert_eq!(en.text("link-name-alias", &[]), "alias");
    }

    /// The folder of the screens, `crates/rf-ui/ui`.
    fn ui_dir() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("ui")
    }

    /// The `.slint` files of the screens, by name, in order, each with its text.
    fn slint_files() -> Vec<(String, String)> {
        let mut files: Vec<(String, String)> = std::fs::read_dir(ui_dir())
            .expect("the screens' folder")
            .map(|entry| entry.expect("an entry").path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "slint"))
            .map(|path| {
                let name = path
                    .file_name()
                    .expect("a file name")
                    .to_string_lossy()
                    .into_owned();
                let text = std::fs::read_to_string(&path).expect("a .slint file");
                (name, text)
            })
            .collect();
        files.sort();
        files
    }

    /// The string literals of the Slint source `chars`, comments left out, each with its line:
    /// a literal's text outside its `\{…}` parts, and the literals in those parts on their own.
    fn slint_literals(chars: &[char]) -> Vec<(usize, String)> {
        let mut found = Vec::new();
        let mut i = 0;
        while i < chars.len() {
            match (chars[i], chars.get(i + 1)) {
                ('/', Some('/')) => {
                    while i < chars.len() && chars[i] != '\n' {
                        i += 1;
                    }
                }
                ('/', Some('*')) => {
                    i += 2;
                    while i < chars.len() && !(chars[i] == '*' && chars.get(i + 1) == Some(&'/')) {
                        i += 1;
                    }
                    i += 2;
                }
                ('"', _) => i = slint_string(chars, i, &mut found),
                _ => i += 1,
            }
        }
        found
    }

    /// Reads the Slint string literal of `chars` that opens at `at` into `found` ([`slint_literals`])
    /// and tells where it ends, after its closing quote.
    fn slint_string(chars: &[char], at: usize, found: &mut Vec<(usize, String)>) -> usize {
        let line = chars[..at].iter().filter(|&&c| c == '\n').count() + 1;
        let mut text = String::new();
        let mut i = at + 1;
        while i < chars.len() && chars[i] != '"' {
            if chars[i] == '\\' && chars.get(i + 1) == Some(&'{') {
                // An expression, to its `}`: a string in it is a literal of its own.
                let mut depth = 1;
                i += 2;
                while i < chars.len() && depth > 0 {
                    match chars[i] {
                        '{' => depth += 1,
                        '}' => depth -= 1,
                        '"' => {
                            i = slint_string(chars, i, found);
                            continue;
                        }
                        _ => {}
                    }
                    i += 1;
                }
            } else if chars[i] == '\\' {
                text.extend(chars.get(i..(i + 2).min(chars.len())).unwrap_or_default());
                i += 2;
            } else {
                text.push(chars[i]);
                i += 1;
            }
        }
        found.push((line, text));
        i + 1
    }

    /// The files whose words Task 10b moves to `Tr`, which still hold Hangul literals: each
    /// must hold one, or its allowance goes.
    const SLINT_NOT_YET: [&str; 6] = [
        "conflict-dialog.slint",
        "dialogs.slint",
        "job-panel.slint",
        "job-window.slint",
        "pick-dialog.slint",
        "properties.slint",
    ];

    /// The rule of I18N-01 for the screens: their words come from `Tr` (ui/tr.slint, filled from
    /// `ui.ftl`), so no `.slint` file holds a Hangul string literal (comments may be Korean), but
    /// those [`SLINT_NOT_YET`] allows. `tr.slint` has no values of its own either.
    #[test]
    fn no_slint_file_holds_a_hangul_string_literal() {
        let files = slint_files();
        let names: Vec<&str> = files.iter().map(|(name, _)| name.as_str()).collect();
        for reached in ["tr.slint", "app.slint", "view-menu.slint", "dialogs.slint"] {
            assert!(names.contains(&reached), "{reached}: {names:?}");
        }
        let mut found = Vec::new();
        let mut needed = Vec::new();
        for (name, text) in &files {
            let chars: Vec<char> = text.chars().collect();
            for (line, literal) in slint_literals(&chars) {
                if !literal.chars().any(is_hangul) {
                    continue;
                }
                if SLINT_NOT_YET.contains(&name.as_str()) {
                    needed.push(name.as_str());
                } else {
                    found.push(format!("{name}:{line}: {literal:?}"));
                }
            }
        }
        assert!(
            found.is_empty(),
            "Hangul literals in the screens (give them a key in i18n/<language>/ui.ftl and \
             a property of Tr): {found:#?}"
        );
        for name in SLINT_NOT_YET {
            assert!(
                needed.contains(&name),
                "{name} holds no Hangul literal any more: take it out of SLINT_NOT_YET"
            );
        }
    }

    #[test]
    fn the_slint_scan_reads_interpolations_and_skips_comments() {
        let source = concat!(
            "// \"주석\" ‘하나’\n",
            "/* \"블록\" */ text: \"보임\";\n",
            "label: \"‘\\{root.name}’ 하위\" + \"\\{Tr.a} · \\{f(\"안\")} 끝\";\n",
            "tip: @markdown(\"\\{Tr.b}\"); plain: \"\\\"따옴\\\"\";\n",
        );
        let chars: Vec<char> = source.chars().collect();
        let shown: Vec<(usize, String)> = slint_literals(&chars);
        assert_eq!(
            shown,
            [
                (2, "보임".to_owned()),
                (3, "‘’ 하위".to_owned()),
                (3, "안".to_owned()),
                (3, " ·  끝".to_owned()),
                (4, String::new()),
                (4, "\\\"따옴\\\"".to_owned()),
            ]
        );
    }

    /// The names of the properties of `Tr` as `ui/tr.slint` declares them, read here on their
    /// own (not through the list the build makes of them): every line that declares a property
    /// must be `in property <string> name;`, with no value.
    fn tr_declared() -> Vec<String> {
        let source = std::fs::read_to_string(ui_dir().join("tr.slint")).expect("ui/tr.slint");
        let mut declared = Vec::new();
        for line in source.lines() {
            let code = line.split("//").next().unwrap_or_default().trim();
            if !code.contains("property") {
                continue;
            }
            let name = code
                .strip_prefix("in property <string> ")
                .and_then(|rest| rest.strip_suffix(';'))
                .unwrap_or_else(|| {
                    panic!("tr.slint: {line:?} is not `in property <string> name;`")
                });
            assert!(!name.contains([':', ' ']), "tr.slint: {line:?} has a value");
            declared.push(name.to_owned());
        }
        declared
    }

    /// `Tr`'s properties and `ui.ftl`'s keys are one to one, in both languages (ui/tr.slint read
    /// on its own here, and the list the build makes from it, which fills the windows, agree);
    /// a message of `ui.ftl` takes no argument but `$os`; every property is shown somewhere.
    #[test]
    fn the_window_s_words_and_ui_ftl_are_one_to_one() {
        let declared = tr_declared();
        assert!(declared.len() > 150, "{} properties", declared.len());
        let filled: Vec<String> = crate::ui_bridge::tr_keys().map(str::to_owned).collect();
        assert_eq!(declared, filled, "ui/tr.slint and the build's list of it");
        let declared: BTreeSet<String> = declared.into_iter().collect();
        for lang in Lang::ALL {
            let ui = messages_in(lang, |area| area == "ui");
            let keys: BTreeSet<String> = ui.keys().cloned().collect();
            assert_eq!(
                declared.difference(&keys).collect::<Vec<_>>(),
                Vec::<&String>::new(),
                "properties of Tr without a message in {}/ui.ftl",
                lang.code()
            );
            assert_eq!(
                keys.difference(&declared).collect::<Vec<_>>(),
                Vec::<&String>::new(),
                "messages of {}/ui.ftl without a property of Tr",
                lang.code()
            );
            for (key, names) in &ui {
                assert!(
                    names.iter().all(|name| name == OS_ARG),
                    "{}/ui.ftl: {key} takes {names:?} (Rust makes the words with arguments: screen.ftl)",
                    lang.code()
                );
            }
        }
        // Each is shown: some screen says `Tr.<name>`.
        let screens: String = slint_files()
            .into_iter()
            .filter(|(name, _)| name != "tr.slint")
            .map(|(_, text)| text)
            .collect();
        for name in &declared {
            let used = screens
                .match_indices(&format!("Tr.{name}"))
                .any(|(at, found)| {
                    !screens[at + found.len()..]
                        .starts_with(|c: char| c.is_alphanumeric() || c == '-' || c == '_')
                });
            assert!(used, "Tr.{name} is shown nowhere");
        }
    }

    /// Every Rust file under `dir`, tests and all, but the build's output.
    fn every_rust_file(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries {
            let path = entry.expect("an entry").path();
            if path.is_dir() {
                if path.file_name().is_none_or(|name| name != "target") {
                    every_rust_file(&path, out);
                }
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                out.push(path);
            }
        }
    }

    /// Where the function `name` of the masked source `masked` is, from its `fn` to the end of
    /// its body (in bytes).
    fn fn_body(masked: &str, name: &str) -> Option<std::ops::Range<usize>> {
        let start = masked.find(&format!("fn {name}("))?;
        let open = start + masked[start..].find('{')?;
        let mut depth = 0;
        for (at, c) in masked[open..].char_indices() {
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(start..open + at);
                    }
                }
                _ => {}
            }
        }
        None
    }

    /// The window components and the function of `ui_bridge` that alone makes each, with its
    /// words: a global is per window, so a window made any other way would show no words.
    const MADE_BY: [(&str, &str); 4] = [
        ("MainWindow", "new_main_window"),
        ("NoticeWindow", "new_notice_window"),
        ("JobWindow", "new_job_window"),
        ("JobTray", "new_job_tray"),
    ];

    /// Every window is made with its words: nothing in the app or its tests (code, not
    /// comments) makes a window component but its maker in `ui_bridge` ([`MADE_BY`]).
    #[test]
    fn every_window_is_made_with_its_words() {
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let workspace = manifest
            .parent()
            .and_then(std::path::Path::parent)
            .expect("the workspace");
        let mut files = Vec::new();
        every_rust_file(&workspace.join("crates"), &mut files);
        every_rust_file(&workspace.join("apps"), &mut files);
        let bridge = manifest.join("src").join("ui_bridge.rs");
        assert!(files.contains(&bridge));
        assert!(files.contains(&manifest.join("tests").join("ui_smoke.rs")));
        let mut found = Vec::new();
        let mut makers = 0;
        for path in &files {
            let source = std::fs::read_to_string(path).expect("a Rust file");
            let chars: Vec<char> = source.chars().collect();
            let masked: String = scan(&chars).masked.into_iter().collect();
            for (component, maker) in MADE_BY {
                let call = format!("{component}::new(");
                for (at, _) in masked.match_indices(&call) {
                    if masked[..at].ends_with(|c: char| c.is_alphanumeric() || c == '_') {
                        continue;
                    }
                    let in_maker = *path == bridge
                        && fn_body(&masked, maker).is_some_and(|body| body.contains(&at));
                    if in_maker {
                        makers += 1;
                    } else {
                        let line = masked[..at].matches('\n').count() + 1;
                        found.push(format!("{}:{line}: {call}", path.display()));
                    }
                }
            }
        }
        assert!(
            found.is_empty(),
            "windows made without their words (call ui_bridge::{{new_main_window, \
             new_notice_window, new_job_window, new_job_tray}}): {found:#?}"
        );
        assert_eq!(makers, MADE_BY.len());
    }

    /// The menus and buttons of the window that English messages name: the message, then the
    /// `Tr` keys of the path it names, joined by " ▸ ". The Korean message names the same path,
    /// so the window's titles and the messages agree in both languages.
    const NAMED_IN_MESSAGES: [(&str, &[&str]); 4] = [
        (
            "tab-moved-to-hidden",
            &["toolbar-view", "view-layout", "view-dual-pane"],
        ),
        ("failure-original-folder-gone", &["command-restore-to"]),
        ("trash-place-unknown", &["command-restore-to"]),
        ("computer-drive-gone", &["command-refresh"]),
    ];

    /// What messages name that Task 10b's files still title with a Hangul literal: the message,
    /// the file, the literal. Once the file titles it from `Tr`, its entry goes to
    /// [`NAMED_IN_MESSAGES`].
    const NAMED_NOT_YET: [(&str, &str, &str); 4] = [
        (
            "tray-all-done-with-failures",
            "job-panel.slint",
            "최근 완료",
        ),
        ("tray-all-done-with-failures", "job-panel.slint", "자세히"),
        ("pattern-uncounted", "dialogs.slint", "선택"),
        ("removal-with-jobs", "dialogs.slint", "취소"),
    ];

    /// Messages whose " ▸ " path is the system's own settings, not the app's menus.
    const NAMES_THE_SYSTEM_S: [&str; 1] = ["shell-finder-not-allowed"];

    /// Every menu name an English message quotes is the title of that menu in the window, as is
    /// its Korean name in the Korean message, on every system.
    #[test]
    fn the_menus_the_messages_name_are_titled_so() {
        let korean = Localizer::new(Lang::Ko);
        let english = Localizer::new(Lang::En);
        let all = messages(Lang::En);
        let ui = messages_in(Lang::En, |area| area == "ui");
        let said = |words: &Localizer, key: &str, os: Os| {
            let mut args: Vec<Arg<'_>> = all[key]
                .iter()
                .map(|name| (name.as_str(), FluentValue::from("x")))
                .collect();
            args.push((OS_ARG, os_arg(os)));
            words.text(key, &args)
        };
        for os in [Os::Windows, Os::MacOs, Os::Other] {
            for (message, path) in NAMED_IN_MESSAGES {
                for words in [&korean, &english] {
                    for key in path {
                        assert!(ui.contains_key(*key), "{key} is not a word of the window");
                    }
                    let titles: Vec<String> = path.iter().map(|key| said(words, key, os)).collect();
                    let named = titles.join(" ▸ ");
                    let text = said(words, message, os);
                    assert!(
                        text.contains(&named),
                        "{}: {message} ({os:?}) does not name {named:?}: {text:?}",
                        words.lang().code()
                    );
                }
            }
            // A path in a message is one of these.
            for key in all.keys() {
                let text = said(&english, key, os);
                let listed = NAMED_IN_MESSAGES.iter().any(|(message, _)| message == key)
                    || NAMES_THE_SYSTEM_S.contains(&key.as_str());
                assert!(
                    !text.contains('▸') || listed,
                    "{key} names a menu: list it in NAMED_IN_MESSAGES ({text:?})"
                );
            }
        }
        // The names in 10b's files: still literals there, and in the Korean messages.
        let files = slint_files();
        for (message, file, title) in NAMED_NOT_YET {
            assert!(
                said(&korean, message, Os::Other).contains(title),
                "{message}"
            );
            let (_, text) = files
                .iter()
                .find(|(name, _)| name == file)
                .unwrap_or_else(|| panic!("no {file}"));
            let chars: Vec<char> = text.chars().collect();
            assert!(
                slint_literals(&chars)
                    .iter()
                    .any(|(_, literal)| literal == title),
                "{file} titles {title:?} from Tr now: move {message} to NAMED_IN_MESSAGES"
            );
        }
    }
}
