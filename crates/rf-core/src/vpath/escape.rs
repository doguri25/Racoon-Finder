//! Lossless text form for names that are not valid text: `%XX` escapes for the bytes
//! that cannot appear literally. Valid UTF-8 (including non-ASCII) stays as it is.

use std::ffi::{OsStr, OsString};

use crate::error::PathError;

const HEX_UPPER: &[u8; 16] = b"0123456789ABCDEF";

fn push_escaped(out: &mut String, byte: u8) {
    out.push('%');
    out.push(char::from(HEX_UPPER[usize::from(byte >> 4)]));
    out.push(char::from(HEX_UPPER[usize::from(byte & 0x0F)]));
}

/// Escapes `bytes` for the string form. Valid UTF-8 text is kept, except `%`, ASCII
/// control characters and the characters in `extra`, which become `%XX` per UTF-8 byte
/// (uppercase hex). Every byte that is not valid UTF-8 becomes its own `%XX`.
pub(crate) fn escape_bytes(bytes: &[u8], extra: &[char]) -> String {
    let mut out = String::with_capacity(bytes.len());
    for chunk in bytes.utf8_chunks() {
        for c in chunk.valid().chars() {
            if c == '%' || c.is_ascii_control() || extra.contains(&c) {
                for byte in c.encode_utf8(&mut [0; 4]).bytes() {
                    push_escaped(&mut out, byte);
                }
            } else {
                out.push(c);
            }
        }
        for &byte in chunk.invalid() {
            push_escaped(&mut out, byte);
        }
    }
    out
}

fn hex_value(digit: u8) -> Option<u8> {
    match digit {
        b'0'..=b'9' => Some(digit - b'0'),
        b'a'..=b'f' => Some(digit - b'a' + 10),
        b'A'..=b'F' => Some(digit - b'A' + 10),
        _ => None,
    }
}

/// Inverse of [`escape_bytes`]. A `%` must be followed by two ASCII hex digits (either
/// case); anything else is `PathError::InvalidEscape` carrying the whole input.
pub(crate) fn unescape_bytes(s: &str) -> Result<Vec<u8>, PathError> {
    let invalid = || PathError::InvalidEscape(s.to_owned());
    let mut out = Vec::with_capacity(s.len());
    let mut rest = s.as_bytes();
    while let Some((&byte, after)) = rest.split_first() {
        if byte == b'%' {
            let &[high, low, ..] = after else {
                return Err(invalid());
            };
            let (Some(high), Some(low)) = (hex_value(high), hex_value(low)) else {
                return Err(invalid());
            };
            out.push((high << 4) | low);
            rest = &after[2..];
        } else {
            out.push(byte);
            rest = after;
        }
    }
    Ok(out)
}

/// [`escape_bytes`] over the platform's own string bytes: raw bytes on Unix.
#[cfg(unix)]
pub(crate) fn escape_os(s: &OsStr, extra: &[char]) -> String {
    use std::os::unix::ffi::OsStrExt;

    escape_bytes(s.as_bytes(), extra)
}

/// [`escape_bytes`] over the platform's own string bytes: WTF-8 of the UTF-16 units on Windows,
/// so an unpaired surrogate becomes a `%ED%A0%80`-style triple.
#[cfg(windows)]
pub(crate) fn escape_os(s: &OsStr, extra: &[char]) -> String {
    use std::os::windows::ffi::OsStrExt;

    let units: Vec<u16> = s.encode_wide().collect();
    escape_bytes(&crate::winpath::to_wtf8(&units), extra)
}

/// Inverse of [`escape_os`].
#[cfg(unix)]
pub(crate) fn unescape_os(s: &str) -> Result<OsString, PathError> {
    use std::os::unix::ffi::OsStringExt;

    Ok(OsString::from_vec(unescape_bytes(s)?))
}

/// Inverse of [`escape_os`]. Bytes that are not WTF-8 are `PathError::InvalidEscape`.
#[cfg(windows)]
pub(crate) fn unescape_os(s: &str) -> Result<OsString, PathError> {
    use std::os::windows::ffi::OsStringExt;

    let units = crate::winpath::from_wtf8(&unescape_bytes(s)?)
        .ok_or_else(|| PathError::InvalidEscape(s.to_owned()))?;
    Ok(OsString::from_wide(&units))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn escape_keeps_text_and_escapes_percent_controls_and_invalid_bytes() {
        assert_eq!(escape_bytes("한글 a.txt".as_bytes(), &[]), "한글 a.txt");
        assert_eq!(escape_bytes(b"100%.txt", &[]), "100%25.txt");
        assert_eq!(escape_bytes(b"a\nb\x7f", &[]), "a%0Ab%7F");
        assert_eq!(escape_bytes(b"\xff\xfe", &[]), "%FF%FE");
        assert_eq!(escape_bytes(b"a/b", &['/']), "a%2Fb");
    }

    #[test]
    fn unescape_rejects_bad_escapes() {
        assert_eq!(
            unescape_bytes("a%2"),
            Err(PathError::InvalidEscape("a%2".into()))
        );
        assert_eq!(
            unescape_bytes("a%zz"),
            Err(PathError::InvalidEscape("a%zz".into()))
        );
        assert_eq!(
            unescape_bytes("a%+1"),
            Err(PathError::InvalidEscape("a%+1".into()))
        );
        assert_eq!(unescape_bytes("%41%e4"), Ok(vec![b'A', 0xE4]));
    }

    proptest! {
        #[test]
        fn escape_roundtrips_any_bytes(b in proptest::collection::vec(any::<u8>(), 0..64)) {
            prop_assert_eq!(unescape_bytes(&escape_bytes(&b, &['/'])).unwrap(), b);
        }
    }
}
