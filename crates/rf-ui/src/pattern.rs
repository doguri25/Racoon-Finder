//! Patterns that choose rows by their names (NAV-15 패턴으로 선택): wildcards or a regular
//! expression, ignoring case or not. The UI thread only builds a pattern (and says why a text is
//! none); a worker matches the names (`state::MatchRequest`).

use std::ffi::OsStr;

use rf_core::Os;
use rf_vfs::{NameFolding, name_text};

/// How the text of a [`NamePattern`] is written.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum PatternSyntax {
    /// Wildcards, as the systems' file dialogs take them: `*` stands for any characters, `?`
    /// for one, and `;` goes between patterns (`*.jpg;*.png`). A pattern matches a whole name;
    /// the spaces around it are not part of it. A name with a `;` of its own is matched with a
    /// `?` or a `*` in its place (`a?b.txt` for `a;b.txt`): a `;` always ends a pattern.
    #[default]
    Wildcard,
    /// A regular expression (the syntax of the `regex` crate), found anywhere in a name.
    Regex,
}

/// Why a text is no [`NamePattern`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PatternError {
    /// It is longer than [`MAX_PATTERN_CHARS`].
    TooLong,
    /// It is no regular expression.
    Invalid,
    /// It would take more memory to match than [`PATTERN_SIZE_LIMIT`].
    TooComplex,
}

/// The most characters a pattern has. Names are at most 255 characters on the systems' file
/// systems, so a pattern this long already says more than any name needs.
pub const MAX_PATTERN_CHARS: usize = 1_000;

/// The most memory, in bytes, a pattern compiles to, and its lazy DFA may take: the `regex`
/// crate matches in time linear in the name, and this keeps the factor of the pattern small.
pub const PATTERN_SIZE_LIMIT: usize = 1 << 20;

/// A pattern the names of rows are matched against (NAV-15 패턴 선택): wildcards or a regular
/// expression ([`PatternSyntax`]), ignoring case or not. A name that is not Unicode is matched
/// as the screen shows it, its bytes that are no text replaced.
///
/// On macOS, whose volumes take the two Unicode forms of a name (NFC and NFD) for one name, the
/// pattern and each name are matched in NFC (`rf_vfs::name_text`): a `?` stands for a whole
/// syllable of a decomposed Korean name, and a composed name typed in matches it.
#[derive(Clone, Debug)]
pub struct NamePattern {
    regex: regex::Regex,
    /// The rule of the system the names are matched by; only its Unicode form counts here, as
    /// case is the pattern's own choice.
    folding: NameFolding,
}

impl PartialEq for NamePattern {
    fn eq(&self, other: &NamePattern) -> bool {
        // The flags are written into the text.
        self.regex.as_str() == other.regex.as_str() && self.folding == other.folding
    }
}

impl Eq for NamePattern {}

impl NamePattern {
    /// The pattern `text` writes in `syntax`, ignoring case when `ignore_case`, for the names of
    /// `os` (composed to NFC on macOS, with the names it matches); `Ok(None)` for one with
    /// nothing to match (an empty text, or wildcards of nothing but `;` and spaces).
    pub fn new(
        text: &str,
        syntax: PatternSyntax,
        ignore_case: bool,
        os: Os,
    ) -> Result<Option<NamePattern>, PatternError> {
        if text.chars().count() > MAX_PATTERN_CHARS {
            return Err(PatternError::TooLong);
        }
        let folding = NameFolding::of(os);
        let text = name_text(OsStr::new(text), folding);
        let text = text.as_ref();
        let body = match syntax {
            PatternSyntax::Wildcard => {
                let parts: Vec<String> = text
                    .split(';')
                    .map(str::trim)
                    .filter(|part| !part.is_empty())
                    .map(wildcard_regex)
                    .collect();
                if parts.is_empty() {
                    return Ok(None);
                }
                format!("^(?:{})$", parts.join("|"))
            }
            PatternSyntax::Regex if text.is_empty() => return Ok(None),
            PatternSyntax::Regex => text.to_owned(),
        };
        // `s`: a `.` and a `*` stand for a line break too, which a name may have on Unix.
        let flags = if ignore_case { "(?is)" } else { "(?s)" };
        let regex = regex::RegexBuilder::new(&format!("{flags}{body}"))
            .size_limit(PATTERN_SIZE_LIMIT)
            .dfa_size_limit(PATTERN_SIZE_LIMIT)
            .build()
            .map_err(|error| match error {
                regex::Error::CompiledTooBig(_) => PatternError::TooComplex,
                _ => PatternError::Invalid,
            })?;
        Ok(Some(NamePattern { regex, folding }))
    }

    /// Whether `name` matches, in the Unicode form of the pattern's system.
    pub fn matches(&self, name: &OsStr) -> bool {
        self.regex.is_match(&name_text(name, self.folding))
    }
}

/// The regular expression of one wildcard pattern: `*` any characters (a run of them one), `?`
/// one, anything else itself.
fn wildcard_regex(pattern: &str) -> String {
    let mut regex = String::with_capacity(pattern.len() * 2);
    let mut last = None;
    for c in pattern.chars() {
        match c {
            '*' if last == Some('*') => {}
            '*' => regex.push_str(".*"),
            '?' => regex.push('.'),
            _ => regex.push_str(&regex::escape(c.encode_utf8(&mut [0; 4]))),
        }
        last = Some(c);
    }
    regex
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pattern(text: &str, syntax: PatternSyntax, ignore_case: bool) -> NamePattern {
        pattern_on(text, syntax, ignore_case, Os::Other)
    }

    fn pattern_on(text: &str, syntax: PatternSyntax, ignore_case: bool, os: Os) -> NamePattern {
        NamePattern::new(text, syntax, ignore_case, os)
            .expect("a pattern")
            .expect("something to match")
    }

    #[test]
    fn on_macos_a_pattern_and_a_name_are_matched_in_one_unicode_form() {
        // 한글 decomposed (NFD), as HFS+ and some Mac apps write names, and composed (NFC).
        let nfd = "\u{1112}\u{1161}\u{11ab}\u{1100}\u{1173}\u{11af}.txt";
        let nfc = "한글.txt";
        for syntax in [PatternSyntax::Wildcard, PatternSyntax::Regex] {
            let two = match syntax {
                PatternSyntax::Wildcard => "??.txt",
                PatternSyntax::Regex => "^..\\.txt$",
            };
            // `?` (or `.`) is a whole syllable on macOS, whichever form the name has.
            let mac = pattern_on(two, syntax, true, Os::MacOs);
            assert_eq!(matching(&mac, &[nfd, nfc]), [nfd, nfc], "{syntax:?}");
            // Elsewhere a decomposed name is the six jamo it is written as.
            let other = pattern_on(two, syntax, true, Os::Windows);
            assert_eq!(matching(&other, &[nfd, nfc]), [nfc], "{syntax:?}");
        }
        // A composed name typed in finds the decomposed one on macOS, and a decomposed one the
        // composed one; elsewhere each finds only its own form.
        let typed = pattern_on("한글*", PatternSyntax::Wildcard, false, Os::MacOs);
        assert_eq!(matching(&typed, &[nfd, nfc]), [nfd, nfc]);
        let decomposed = pattern_on(
            "\u{1112}\u{1161}\u{11ab}*",
            PatternSyntax::Wildcard,
            false,
            Os::MacOs,
        );
        assert_eq!(matching(&decomposed, &[nfd, nfc]), [nfd, nfc]);
        let typed = pattern_on("한글*", PatternSyntax::Wildcard, false, Os::Other);
        assert_eq!(matching(&typed, &[nfd, nfc]), [nfc]);
    }

    fn matching(pattern: &NamePattern, names: &[&str]) -> Vec<String> {
        names
            .iter()
            .filter(|name| pattern.matches(OsStr::new(name)))
            .map(|&name| name.to_owned())
            .collect()
    }

    const PHOTOS: [&str; 7] = [
        "a.jpg",
        "B.JPG",
        "c.png",
        "d.jpg.txt",
        "사진 1.jpeg",
        "x.jp",
        "jpg",
    ];

    #[test]
    fn wildcards_match_whole_names_ignoring_case_by_choice() {
        let both = pattern("*.jpg;*.png", PatternSyntax::Wildcard, true);
        assert_eq!(matching(&both, &PHOTOS), ["a.jpg", "B.JPG", "c.png"]);
        // Spaces around a pattern and empty patterns between `;` are nothing.
        let spaced = pattern(" *.jpg ; ;*.png ", PatternSyntax::Wildcard, true);
        assert_eq!(spaced, both);
        // Case counts when asked to.
        let exact = pattern("*.jpg", PatternSyntax::Wildcard, false);
        assert_eq!(matching(&exact, &PHOTOS), ["a.jpg"]);
        // `?` is one character; anything else is itself, regex characters too.
        let one = pattern("?.jp?", PatternSyntax::Wildcard, true);
        assert_eq!(matching(&one, &PHOTOS), ["a.jpg", "B.JPG"]);
        let words = pattern("사진 ?.jpeg", PatternSyntax::Wildcard, true);
        assert_eq!(matching(&words, &PHOTOS), ["사진 1.jpeg"]);
        let literal = pattern("a+(1).[x]", PatternSyntax::Wildcard, true);
        assert_eq!(matching(&literal, &["a+(1).[x]", "aa(1).x"]), ["a+(1).[x]"]);
        // A pattern without wildcards is a whole name.
        let name = pattern("jpg", PatternSyntax::Wildcard, true);
        assert_eq!(matching(&name, &PHOTOS), ["jpg"]);
        // A run of stars is one.
        assert_eq!(
            pattern("***.png", PatternSyntax::Wildcard, true),
            pattern("*.png", PatternSyntax::Wildcard, true)
        );
        // A name with a line break, which Unix allows.
        let star = pattern("a*", PatternSyntax::Wildcard, true);
        assert!(star.matches(OsStr::new("a\nb")));
    }

    #[test]
    fn a_regular_expression_is_found_anywhere_in_a_name() {
        let jpg = pattern(r"\.jpe?g$", PatternSyntax::Regex, true);
        assert_eq!(matching(&jpg, &PHOTOS), ["a.jpg", "B.JPG", "사진 1.jpeg"]);
        let exact = pattern(r"\.jpe?g$", PatternSyntax::Regex, false);
        assert_eq!(matching(&exact, &PHOTOS), ["a.jpg", "사진 1.jpeg"]);
        let part = pattern("jp", PatternSyntax::Regex, true);
        assert_eq!(matching(&part, &PHOTOS).len(), 6);
        // `;` is the regex's own.
        let semicolon = pattern("a;b", PatternSyntax::Regex, true);
        assert_eq!(matching(&semicolon, &["a;b", "a", "b"]), ["a;b"]);
    }

    #[test]
    fn a_text_that_is_no_pattern_says_why() {
        for syntax in [PatternSyntax::Wildcard, PatternSyntax::Regex] {
            assert_eq!(NamePattern::new("", syntax, true, Os::Other), Ok(None));
            let long = "a".repeat(MAX_PATTERN_CHARS + 1);
            assert_eq!(
                NamePattern::new(&long, syntax, true, Os::Other),
                Err(PatternError::TooLong)
            );
            let longest = "가".repeat(MAX_PATTERN_CHARS);
            assert!(matches!(
                NamePattern::new(&longest, syntax, true, Os::Other),
                Ok(Some(_))
            ));
        }
        assert_eq!(
            NamePattern::new(" ; ", PatternSyntax::Wildcard, true, Os::Other),
            Ok(None)
        );
        for bad in ["(", "[a-", "*.jpg", "a{2,1}", r"\p{Nope}"] {
            assert_eq!(
                NamePattern::new(bad, PatternSyntax::Regex, true, Os::Other),
                Err(PatternError::Invalid),
                "{bad}"
            );
        }
        // Short, but it would compile to more than the limit.
        assert_eq!(
            NamePattern::new(
                r"\w{200}\w{200}\w{200}",
                PatternSyntax::Regex,
                true,
                Os::Other
            ),
            Err(PatternError::TooComplex)
        );
        // The same text in wildcards is a name.
        assert!(matches!(
            NamePattern::new("(", PatternSyntax::Wildcard, true, Os::Other),
            Ok(Some(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn a_name_that_is_no_text_is_matched_as_it_shows() {
        use std::os::unix::ffi::OsStrExt;
        let name = OsStr::from_bytes(b"photo\xff.jpg");
        assert!(pattern("photo?.jpg", PatternSyntax::Wildcard, true).matches(name));
    }

    #[test]
    fn case_is_ignored_beyond_ascii() {
        let accented = pattern(r"é\.jpg$", PatternSyntax::Regex, true);
        assert_eq!(
            matching(&accented, &["É.JPG", "é.jpg", "e.jpg"]),
            ["É.JPG", "é.jpg"]
        );
        let exact = pattern(r"é\.jpg$", PatternSyntax::Regex, false);
        assert_eq!(matching(&exact, &["É.JPG", "é.jpg"]), ["é.jpg"]);
        let wild = pattern("é*", PatternSyntax::Wildcard, true);
        assert_eq!(matching(&wild, &["É.JPG", "Ω.jpg"]), ["É.JPG"]);
        let greek = pattern("ω*", PatternSyntax::Wildcard, true);
        assert_eq!(matching(&greek, &["Ω.jpg"]), ["Ω.jpg"]);
    }

    #[test]
    fn a_semicolon_of_a_name_is_matched_by_a_wildcard_in_its_place() {
        let names = ["a;b.txt", "a", "b.txt", "axb.txt"];
        // `;` ends a pattern: two patterns, neither of them the name with the `;`.
        let split = pattern("a;b.txt", PatternSyntax::Wildcard, true);
        assert_eq!(matching(&split, &names), ["a", "b.txt"]);
        let one = pattern("a?b.txt", PatternSyntax::Wildcard, true);
        assert_eq!(matching(&one, &names), ["a;b.txt", "axb.txt"]);
        let any = pattern("a*b.txt", PatternSyntax::Wildcard, true);
        assert_eq!(matching(&any, &names), ["a;b.txt", "axb.txt"]);
    }
}
