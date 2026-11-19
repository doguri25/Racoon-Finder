//! Windows path-prefix rules as pure functions over UTF-16 code units; compiled on every OS so Linux CI tests them.

/// Longest path, in UTF-16 code units, that the extended-length (`\\?\`) form can carry.
/// `to_extended_length` does not check it; callers use it as their upper bound.
pub const MAX_EXTENDED_PATH: usize = 32_767;

const BACKSLASH: u16 = b'\\' as u16;
const SLASH: u16 = b'/' as u16;
const COLON: u16 = b':' as u16;
const QUESTION: u16 = b'?' as u16;
const DOT: u16 = b'.' as u16;

/// Code points `U+D800..=U+DFFF`: valid as a WTF-8 unit but not as a `char`.
const SURROGATES: std::ops::RangeInclusive<u32> = 0xD800..=0xDFFF;
const HIGH_SURROGATES: std::ops::RangeInclusive<u16> = 0xD800..=0xDBFF;
const LOW_SURROGATES: std::ops::RangeInclusive<u16> = 0xDC00..=0xDFFF;

fn is_separator(unit: u16) -> bool {
    unit == BACKSLASH || unit == SLASH
}

fn is_drive_letter(unit: u16) -> bool {
    u8::try_from(unit).is_ok_and(|b| b.is_ascii_alphabetic())
}

/// Whether `path` starts with the ASCII `prefix`, comparing letters without regard to case.
fn starts_with_ascii_ignore_case(path: &[u16], prefix: &str) -> bool {
    path.len() >= prefix.len()
        && path.iter().zip(prefix.bytes()).all(|(&unit, expected)| {
            u8::try_from(unit).is_ok_and(|b| b.eq_ignore_ascii_case(&expected))
        })
}

/// Converts an absolute Windows path to the extended-length form that lifts the
/// `MAX_PATH` limit and stops the Win32 layer from rewriting names.
///
/// - `X:\...` or `X:/...` becomes `\\?\X:\...`, and `\\server\share\...` becomes
///   `\\?\UNC\server\share\...`; every `/` turns into `\`.
/// - Paths that already start with `\\?\` or `\\.\` (either separator) are returned unchanged.
/// - Anything else (empty, relative, drive-relative such as `C:a`, rooted such as `\a`) is `None`.
///
/// The length is not checked; see [`MAX_EXTENDED_PATH`].
pub fn to_extended_length(path: &[u16]) -> Option<Vec<u16>> {
    if matches!(
        *path,
        [a, b, c, d, ..] if is_separator(a) && is_separator(b) && (c == QUESTION || c == DOT) && is_separator(d)
    ) {
        return Some(path.to_vec());
    }

    let normalized: Vec<u16> = path
        .iter()
        .map(|&unit| if unit == SLASH { BACKSLASH } else { unit })
        .collect();
    match *normalized.as_slice() {
        [drive, COLON, BACKSLASH, ..] if is_drive_letter(drive) => {
            let mut out: Vec<u16> = r"\\?\".encode_utf16().collect();
            out.extend_from_slice(&normalized);
            Some(out)
        }
        [BACKSLASH, BACKSLASH, third, ..] if third != BACKSLASH => {
            let mut out: Vec<u16> = r"\\?\UNC\".encode_utf16().collect();
            out.extend_from_slice(&normalized[2..]);
            Some(out)
        }
        _ => None,
    }
}

/// Inverse of [`to_extended_length`] for display: `\\?\UNC\srv\share` becomes
/// `\\srv\share` and `\\?\C:\a` becomes `C:\a`. Every other path, including
/// other verbatim forms such as `\\?\Volume{...}\`, is returned unchanged.
pub fn strip_extended_length(path: &[u16]) -> Vec<u16> {
    if starts_with_ascii_ignore_case(path, r"\\?\UNC\") {
        let mut out: Vec<u16> = r"\\".encode_utf16().collect();
        out.extend_from_slice(&path[8..]);
        return out;
    }
    if starts_with_ascii_ignore_case(path, r"\\?\")
        && matches!(path[4..], [drive, COLON, BACKSLASH, ..] if is_drive_letter(drive))
    {
        return path[4..].to_vec();
    }
    path.to_vec()
}

/// Low byte of a value that the caller has already reduced to at most 8 bits.
fn byte(value: u16) -> u8 {
    u8::try_from(value).expect("value is reduced to 8 bits by the caller")
}

/// Encodes UTF-16 code units as WTF-8: ordinary UTF-8 for well-formed text, plus the
/// generalized 3-byte form for each unpaired surrogate, so any Windows name round-trips.
pub fn to_wtf8(units: &[u16]) -> Vec<u8> {
    let mut out = Vec::with_capacity(units.len());
    for decoded in char::decode_utf16(units.iter().copied()) {
        match decoded {
            Ok(c) => out.extend_from_slice(c.encode_utf8(&mut [0; 4]).as_bytes()),
            Err(e) => {
                let unit = e.unpaired_surrogate();
                out.push(byte(0xE0 | (unit >> 12)));
                out.push(byte(0x80 | ((unit >> 6) & 0x3F)));
                out.push(byte(0x80 | (unit & 0x3F)));
            }
        }
    }
    out
}

/// Decodes WTF-8 back to UTF-16 code units. Returns `None` for truncated sequences,
/// stray continuation bytes, overlong forms, code points above `U+10FFFF`, and a surrogate
/// pair written as two 3-byte sequences (the pair must be one 4-byte sequence), so every
/// accepted byte string is exactly what [`to_wtf8`] produces for the decoded units.
pub fn from_wtf8(bytes: &[u8]) -> Option<Vec<u16>> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut rest = bytes;
    while let Some((&lead, after_lead)) = rest.split_first() {
        // (sequence length, payload bits of the lead byte, smallest code point that needs this length)
        let (len, payload, min) = match lead {
            0x00..=0x7F => (1, lead, 0),
            0xC0..=0xDF => (2, lead & 0x1F, 0x80),
            0xE0..=0xEF => (3, lead & 0x0F, 0x800),
            0xF0..=0xF7 => (4, lead & 0x07, 0x1_0000),
            _ => return None,
        };
        let tail = after_lead.get(..len - 1)?;
        let mut code_point = u32::from(payload);
        for &b in tail {
            if b & 0xC0 != 0x80 {
                return None;
            }
            code_point = (code_point << 6) | u32::from(b & 0x3F);
        }
        if code_point < min {
            return None;
        }
        match char::from_u32(code_point) {
            Some(c) => out.extend_from_slice(c.encode_utf16(&mut [0; 2])),
            None if SURROGATES.contains(&code_point) => {
                let unit = u16::try_from(code_point).ok()?;
                // Only a 3-byte sequence can leave a high surrogate as the last unit (a 4-byte
                // one pushes its pair whole). A low surrogate right after it would be a pair
                // spelled as two 3-byte sequences, which WTF-8 forbids: it must be one 4-byte
                // sequence, or two byte strings would decode to the same path.
                let after_high = out
                    .last()
                    .is_some_and(|prev| HIGH_SURROGATES.contains(prev));
                if after_high && LOW_SURROGATES.contains(&unit) {
                    return None;
                }
                out.push(unit);
            }
            None => return None,
        }
        rest = after_lead.get(len - 1..)?;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn w(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }
    fn s(u: &[u16]) -> String {
        String::from_utf16(u).unwrap()
    }
    fn ext(p: &str) -> Option<String> {
        to_extended_length(&w(p)).map(|u| s(&u))
    }

    #[test]
    fn extended_length_of_drive_and_unc_paths() {
        assert_eq!(ext(r"C:\a\b").as_deref(), Some(r"\\?\C:\a\b"));
        assert_eq!(ext("c:/a/b").as_deref(), Some(r"\\?\c:\a\b"));
        assert_eq!(ext(r"C:\").as_deref(), Some(r"\\?\C:\"));
        assert_eq!(
            ext(r"\\srv\share\x").as_deref(),
            Some(r"\\?\UNC\srv\share\x")
        );
        assert_eq!(
            ext("//srv/share/x").as_deref(),
            Some(r"\\?\UNC\srv\share\x")
        );
    }

    #[test]
    fn extended_length_keeps_verbatim_and_device_paths() {
        assert_eq!(ext(r"\\?\C:\a").as_deref(), Some(r"\\?\C:\a"));
        assert_eq!(ext(r"\\.\COM1").as_deref(), Some(r"\\.\COM1"));
    }

    #[test]
    fn extended_length_rejects_relative_forms() {
        for p in ["", r"a\b", "C:", "C:a", r"\a", "a"] {
            assert_eq!(ext(p), None, "{p}");
        }
    }

    #[test]
    fn extended_length_handles_32k_paths_and_lone_surrogates() {
        let mut long = w(r"C:\");
        long.extend(std::iter::repeat_n(u16::from(b'a'), MAX_EXTENDED_PATH - 7));
        assert_eq!(to_extended_length(&long).unwrap().len(), MAX_EXTENDED_PATH);
        let lone = [u16::from(b'C'), u16::from(b':'), u16::from(b'\\'), 0xD800];
        assert_eq!(
            to_extended_length(&lone).unwrap(),
            [w(r"\\?\C:\").as_slice(), &[0xD800]].concat()
        );
    }

    #[test]
    fn strip_extended_length_inverts_drive_and_unc() {
        assert_eq!(s(&strip_extended_length(&w(r"\\?\C:\a"))), r"C:\a");
        assert_eq!(
            s(&strip_extended_length(&w(r"\\?\UNC\srv\share\a"))),
            r"\\srv\share\a"
        );
        assert_eq!(
            s(&strip_extended_length(&w(r"\\?\unc\srv\share\a"))),
            r"\\srv\share\a"
        );
        assert_eq!(
            s(&strip_extended_length(&w(r"\\?\Volume{1}\a"))),
            r"\\?\Volume{1}\a"
        );
        assert_eq!(s(&strip_extended_length(&w(r"C:\a"))), r"C:\a");
    }

    #[test]
    fn wtf8_encodes_lone_surrogates() {
        assert_eq!(to_wtf8(&[0xD800]), vec![0xED, 0xA0, 0x80]);
        assert_eq!(from_wtf8(&[0xED, 0xA0, 0x80]), Some(vec![0xD800]));
        assert_eq!(from_wtf8(&[0xFF]), None);
        assert_eq!(from_wtf8(&[0xC0, 0x80]), None);
        assert_eq!(from_wtf8(&[0xE2, 0x82]), None);
        assert_eq!(from_wtf8(&[0xF4, 0x90, 0x80, 0x80]), None);
        assert_eq!(from_wtf8("한".as_bytes()), Some(vec![0xD55C]));
    }

    #[test]
    fn wtf8_rejects_a_surrogate_pair_spelled_as_two_3_byte_sequences() {
        // High then low surrogate, each as a 3-byte sequence: non-canonical.
        assert_eq!(from_wtf8(&[0xED, 0xA0, 0x80, 0xED, 0xB0, 0x80]), None);
        // The same pair as one 4-byte sequence is the canonical form.
        assert_eq!(from_wtf8("😀".as_bytes()), Some(vec![0xD83D, 0xDE00]));
        // Unpaired surrogates next to each other or next to a 4-byte character stay valid.
        assert_eq!(
            from_wtf8(&[0xED, 0xB0, 0x80, 0xED, 0xA0, 0x80]),
            Some(vec![0xDC00, 0xD800])
        );
        assert_eq!(
            from_wtf8(&[0xED, 0xA0, 0x80, 0xED, 0xA0, 0x80]),
            Some(vec![0xD800, 0xD800])
        );
        assert_eq!(
            from_wtf8(&[0xED, 0xB0, 0x80, 0xED, 0xB0, 0x80]),
            Some(vec![0xDC00, 0xDC00])
        );
        let mut high_then_emoji = vec![0xED, 0xA0, 0x80];
        high_then_emoji.extend_from_slice("😀".as_bytes());
        assert_eq!(
            from_wtf8(&high_then_emoji),
            Some(vec![0xD800, 0xD83D, 0xDE00])
        );
        let mut emoji_then_low = "😀".as_bytes().to_vec();
        emoji_then_low.extend_from_slice(&[0xED, 0xB0, 0x80]);
        assert_eq!(
            from_wtf8(&emoji_then_low),
            Some(vec![0xD83D, 0xDE00, 0xDC00])
        );
    }

    proptest! {
        #[test]
        fn wtf8_roundtrips_any_units(u in proptest::collection::vec(any::<u16>(), 0..64)) {
            prop_assert_eq!(from_wtf8(&to_wtf8(&u)), Some(u));
        }

        #[test]
        fn wtf8_accepts_only_canonical_bytes(
            b in proptest::collection::vec(
                prop::sample::select(vec![0x41_u8, 0xED, 0xA0, 0xB0, 0x80, 0xBF, 0xF0, 0x9F, 0x98]),
                0..12,
            ),
        ) {
            if let Some(units) = from_wtf8(&b) {
                prop_assert_eq!(to_wtf8(&units), b);
            }
        }
    }
}
