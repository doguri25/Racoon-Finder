//! Natural (logical) filename order: the OS rule on Windows and macOS, a deterministic portable rule elsewhere.

use std::cmp::Ordering;
use std::ffi::OsStr;

/// One piece of a name under the portable rule.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Token {
    Num(usize, String), // (digit count without leading zeros, digits): numeric value order
    Text(char),         // lowercased character; every Num sorts before every Text
}

/// Splits a name into tokens: each run of ASCII digits becomes one `Num` (leading zeros
/// removed), every other character becomes one `Text` per character of its lowercase form.
fn tokens(name: &OsStr) -> Vec<Token> {
    let text = name.to_string_lossy();
    let mut out = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c.is_ascii_digit() {
            let mut digits = String::from(c);
            while let Some(next) = chars.next_if(char::is_ascii_digit) {
                digits.push(next);
            }
            let significant = digits.trim_start_matches('0');
            out.push(Token::Num(significant.len(), significant.to_owned()));
        } else {
            out.extend(c.to_lowercase().map(Token::Text));
        }
    }
    out
}

#[cfg(windows)]
mod imp {
    #![allow(unsafe_code, reason = "StrCmpLogicalW FFI")]

    use std::cmp::Ordering;
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;

    use windows::Win32::UI::Shell::StrCmpLogicalW;
    use windows::core::PCWSTR;

    /// The name as NUL-terminated UTF-16.
    #[derive(Debug)]
    pub struct Key(Vec<u16>);

    impl Key {
        pub fn new(name: &OsStr) -> Self {
            let mut wide: Vec<u16> = name.encode_wide().collect();
            wide.push(0);
            Self(wide)
        }

        pub fn compare(&self, other: &Self) -> Ordering {
            // SAFETY: both buffers end in a NUL and stay alive and unmoved for the whole call.
            let result =
                unsafe { StrCmpLogicalW(PCWSTR(self.0.as_ptr()), PCWSTR(other.0.as_ptr())) };
            result.cmp(&0)
        }
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use std::cmp::Ordering;
    use std::ffi::OsStr;

    use objc2::rc::Retained;
    use objc2_foundation::NSString;

    /// The name as an `NSString`; APFS names are always UTF-8, so the lossy conversion never
    /// changes one.
    #[derive(Debug)]
    pub struct Key(Retained<NSString>);

    impl Key {
        pub fn new(name: &OsStr) -> Self {
            Self(NSString::from_str(&name.to_string_lossy()))
        }

        pub fn compare(&self, other: &Self) -> Ordering {
            self.0.localizedStandardCompare(&other.0).into()
        }
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
mod imp {
    use std::cmp::Ordering;
    use std::ffi::OsStr;

    use super::{Token, tokens};

    /// The name as portable tokens.
    #[derive(Debug)]
    pub struct Key(Vec<Token>);

    impl Key {
        pub fn new(name: &OsStr) -> Self {
            Self(tokens(name))
        }

        pub fn compare(&self, other: &Self) -> Ordering {
            self.0.cmp(&other.0)
        }
    }
}

/// A name prepared for natural-order comparison. Build it once per entry before sorting.
///
/// Windows compares with `StrCmpLogicalW`, macOS with `localizedStandardCompare`, other
/// systems with the portable rule of [`portable_natural_cmp`]. It does not implement `Ord`:
/// the OS rules are not guaranteed to be total orders, so sorting must go through a
/// comparison that tolerates that.
#[derive(Debug)]
pub struct NaturalKey(imp::Key);

impl NaturalKey {
    pub fn new(name: &OsStr) -> NaturalKey {
        NaturalKey(imp::Key::new(name))
    }

    pub fn compare(&self, other: &NaturalKey) -> Ordering {
        self.0.compare(&other.0)
    }
}

/// Compares two names with the OS natural-order rule.
pub fn natural_cmp(a: &OsStr, b: &OsStr) -> Ordering {
    NaturalKey::new(a).compare(&NaturalKey::new(b))
}

/// Compares two names with a deterministic total order that behaves the same on every OS:
/// case-insensitive, digit runs by numeric value (leading zeros ignored), digits before
/// other characters.
pub fn portable_natural_cmp(a: &OsStr, b: &OsStr) -> Ordering {
    tokens(a).cmp(&tokens(b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use std::cmp::Ordering::{Equal, Greater, Less};

    fn p(a: &str, b: &str) -> Ordering {
        portable_natural_cmp(OsStr::new(a), OsStr::new(b))
    }

    #[test]
    fn ignores_case_and_leading_zeros() {
        assert_eq!(p("File1", "file1"), Equal);
        assert_eq!(p("a01", "a1"), Equal);
        assert_eq!(p("Ä", "ä"), Equal);
        assert_eq!(p("Z", "a"), Greater);
    }

    #[test]
    fn compares_digit_runs_by_value() {
        assert_eq!(p("x9", "x10"), Less);
        assert_eq!(p("img12", "img100"), Less);
        assert_eq!(p("a1b", "a01c"), Less);
        assert_eq!(p("n99999999999999999999", "n100000000000000000000"), Less);
        assert_eq!(p("v0", "v"), Greater);
    }

    #[test]
    fn digits_sort_before_other_characters() {
        assert_eq!(p("a1", "a_"), Less);
        assert_eq!(p("9", "a"), Less);
        assert_eq!(p("a1", "a"), Greater);
    }

    proptest! {
        #[test]
        fn is_a_total_order(
            a in "[a-cA-C0-2가 _]{0,6}",
            b in "[a-cA-C0-2가 _]{0,6}",
            c in "[a-cA-C0-2가 _]{0,6}",
        ) {
            prop_assert_eq!(p(&a, &a), Equal);
            prop_assert_eq!(p(&a, &b), p(&b, &a).reverse());
            if p(&a, &b) != Greater && p(&b, &c) != Greater {
                prop_assert_ne!(p(&a, &c), Greater);
            }
        }
    }
}
