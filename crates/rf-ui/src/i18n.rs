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
//! - A Korean particle (이/가) is chosen in Rust and passed as an argument whose name starts
//!   with `ko-` (`$ko-ga`): only the Korean messages use such arguments.
//! - Every message may use `$os` ([`OS_ARG`]: `windows`, `macos` or `other`), the system the app
//!   runs on, which the caller need not pass: English says "Recycle Bin" on Windows and "Trash"
//!   elsewhere where Korean says 휴지통 on both. A caller that names a system passes `os` itself.
//! - A function that returns `&'static str` asks [`t_static`], for a message without arguments
//!   (other than `$os`).

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
const AREAS: [(&str, &str, &str); 9] = [
    area!("errors"),
    area!("format"),
    area!("jobs"),
    area!("notices"),
    area!("places"),
    area!("removal"),
    area!("shell"),
    area!("startup"),
    area!("status"),
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
        let Some((text, errors)) = self.format(key, args) else {
            note(format_args!("no message {key}"));
            return key.to_owned();
        };
        if !errors.is_empty() {
            note(format_args!("{key}: {errors:?}"));
        }
        text
    }

    /// The message `key` with `args`, and the errors of its formatting, from the first bundle
    /// that has it; `None` when none has.
    fn format(&self, key: &str, args: &[Arg<'_>]) -> Option<(String, Vec<FluentError>)> {
        let pattern = self
            .bundles
            .iter()
            .find_map(|bundle| Some((bundle, bundle.get_message(key)?.value()?)));
        let (bundle, pattern) = pattern?;
        let mut all = FluentArgs::with_capacity(args.len() + 1);
        all.set(OS_ARG, os_arg(Os::CURRENT));
        for (name, value) in args {
            all.set(*name, value.clone());
        }
        let mut errors = Vec::new();
        let text = bundle.format_pattern(pattern, Some(&all), &mut errors);
        Some((text.into_owned(), errors))
    }
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
        let mut all = BTreeMap::new();
        for (area, source) in lang.sources() {
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
                        let (text, errors) = localizer.format(&key, &args).expect("a message");
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

    /// The line in `words.rs` above which its words are moved; below it, 9b's still are not.
    const SPLIT_MARKER: &str = "// ---- i18n: words not yet moved ----";

    /// The files whose Hangul literals are allowed for now, from the marker on when one is
    /// named, else whole: Task 9b moves their words to the Fluent files and removes them here.
    const NOT_YET_MOVED: [(&str, Option<&str>); 2] = [
        ("crates/rf-ui/src/words.rs", Some(SPLIT_MARKER)),
        // The names a new link takes on Windows and macOS (` - 바로 가기.lnk`, ` 별칭`).
        ("crates/rf-platform/src/links/mod.rs", None),
    ];

    /// Test support that is compiled into the library, whose Hangul literals are the names of
    /// the files its checks make, never words of the app.
    const FIXTURES: [&str; 1] = ["crates/rf-vfs/src/testkit.rs"];

    /// Whether `c` is Hangul: a syllable or a jamo.
    fn is_hangul(c: char) -> bool {
        matches!(c,
            '\u{1100}'..='\u{11FF}'
            | '\u{3130}'..='\u{318F}'
            | '\u{A960}'..='\u{A97F}'
            | '\u{AC00}'..='\u{D7A3}'
            | '\u{D7B0}'..='\u{D7FF}')
    }

    /// A Rust file read for its literals: its text with the comments and the literals blanked
    /// out, and the literals, each with the place it starts at.
    struct Scanned {
        masked: Vec<char>,
        literals: Vec<(usize, String)>,
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
                literals.push((i, chars[body..end.min(chars.len())].iter().collect()));
                blank(&mut masked, i, end + 1 + hashes);
                i = end + 1 + hashes;
            } else if c == '"' {
                let mut end = i + 1;
                while end < chars.len() && chars[end] != '"' {
                    end += if chars[end] == '\\' { 2 } else { 1 };
                }
                literals.push((i, chars[i + 1..end.min(chars.len())].iter().collect()));
                blank(&mut masked, i, end + 1);
                i = end + 1;
            } else if c == '\'' && (at(i + 1) == Some('\\') || at(i + 2) == Some('\'')) {
                // A character, not a lifetime.
                let mut end = i + 2;
                while end < chars.len() && chars[end] != '\'' {
                    end += 1;
                }
                literals.push((i, chars[i + 1..end.min(chars.len())].iter().collect()));
                blank(&mut masked, i, end + 1);
                i = end + 1;
            } else {
                i += 1;
            }
        }
        Scanned { masked, literals }
    }

    /// The parts of `masked` that only tests compile: each item under a `#[cfg(…)]` that needs
    /// `test` (`test`, `all(test, unix)`), as places from to; and the names of such items that
    /// are modules in files of their own (`#[cfg(test)] mod tests;`).
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
            let words: Vec<&str> = attribute
                .split(|c: char| !(c.is_alphanumeric() || c == '_'))
                .collect();
            if !words.contains(&"test") || attribute.contains("not(test") {
                continue;
            }
            // The item: to its first `;`, or to the `}` of its first `{`.
            let mut j = i + 1;
            let mut depth = 0;
            while j < masked.len() {
                match masked[j] {
                    '(' | '[' => depth += 1,
                    ')' | ']' => depth -= 1,
                    ';' if depth == 0 => break,
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
            let item: String = masked[i + 1..j.min(masked.len())].iter().collect();
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
    /// Rust code outside tests holds a Hangul string or character literal, but in what
    /// [`NOT_YET_MOVED`] (9b's) and [`FIXTURES`] allow.
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
        for reached in ["crates/rf-ui/src/words.rs", "crates/rf-ui/src/format.rs"] {
            assert!(
                names.iter().any(|name| name == reached),
                "{reached}: {names:?}"
            );
        }
        let mut test_files = Vec::new();
        let mut found: Vec<(String, usize, String)> = Vec::new();
        let mut allowed: Vec<&str> = Vec::new();
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
            let allowed_from = match NOT_YET_MOVED.iter().find(|(file, _)| file == name) {
                Some((_, Some(marker))) => {
                    let at = source
                        .find(marker)
                        .unwrap_or_else(|| panic!("{name} has no marker {marker:?}"));
                    source[..at].chars().count()
                }
                Some((_, None)) => 0,
                None if FIXTURES.contains(&name.as_str()) => 0,
                None => usize::MAX,
            };
            for (at, literal) in scanned.literals {
                let in_test = tests.iter().any(|&(from, to)| (from..to).contains(&at));
                if !literal.chars().any(is_hangul) || in_test {
                    continue;
                }
                if at < allowed_from {
                    let line = chars[..at].iter().filter(|&&c| c == '\n').count() + 1;
                    found.push((name.clone(), line, literal));
                } else {
                    allowed.push(name);
                }
            }
        }
        found.retain(|(name, _, _)| !test_files.contains(name));
        assert!(
            found.is_empty(),
            "Hangul literals outside tests (move the words to crates/rf-ui/i18n): {found:#?}"
        );
        // An allowance goes once its file has no Korean words left.
        for (file, _) in NOT_YET_MOVED {
            assert!(
                allowed.contains(&file),
                "{file} needs no allowance any more: take it out of NOT_YET_MOVED"
            );
        }
    }

    #[test]
    fn the_scan_finds_literals_and_skips_comments_and_tests() {
        let source = "// ‘주석’ \"아님\"\nfn a() { let x = \"보임\"; let c = '가'; let r = r#\"원\"#; }\n/* \"블록\" */\nfn b<'a>(_: &'a str) {}\n#[cfg(all(test, unix))]\nmod t { fn c() { let _ = \"시험\"; } }\n#[cfg(not(test))]\nfn d() { let _ = \"보임2\"; }\n#[cfg(test)]\nmod tests;\n";
        let chars: Vec<char> = source.chars().collect();
        let scanned = scan(&chars);
        let (tests, modules) = test_items(&scanned.masked);
        let shown: Vec<&str> = scanned
            .literals
            .iter()
            .filter(|(at, _)| !tests.iter().any(|&(from, to)| (from..to).contains(at)))
            .map(|(_, literal)| literal.as_str())
            .collect();
        assert_eq!(shown, ["보임", "가", "원", "보임2"]);
        assert_eq!(modules, ["tests"]);
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
    fn every_key_the_moved_code_asks_for_is_in_both_languages() {
        let ko = messages(Lang::Ko);
        let en = messages(Lang::En);
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut asked = 0;
        for file in ["words.rs", "format.rs"] {
            let source = std::fs::read_to_string(src.join(file)).expect("a Rust file");
            // The code above 9b's marker and outside the tests.
            let moved = source.split(SPLIT_MARKER).next().unwrap_or_default();
            let moved = moved.split("#[cfg(test)]").next().unwrap_or_default();
            let chars: Vec<char> = moved.chars().collect();
            for (_, literal) in scan(&chars).literals {
                let is_key = literal.contains('-')
                    && !literal.starts_with('-')
                    && literal
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
                if is_key {
                    asked += 1;
                    assert!(ko.contains_key(&literal), "{file}: {literal} is not in ko");
                    assert!(en.contains_key(&literal), "{file}: {literal} is not in en");
                }
            }
        }
        assert!(asked > 200, "{asked} keys");
    }
}
