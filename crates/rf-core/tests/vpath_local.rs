use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use rf_core::{PathError, Scheme, VPath};

/// Absolute host path: `C:\a\b` on Windows, `/a/b` elsewhere.
fn abs(parts: &[&str]) -> PathBuf {
    let mut p = PathBuf::from(if cfg!(windows) { r"C:\" } else { "/" });
    for part in parts {
        p.push(part);
    }
    p
}

fn local(parts: &[&str]) -> VPath {
    VPath::local(abs(parts)).unwrap()
}

#[test]
fn scheme_names_roundtrip() {
    let names: Vec<&str> = Scheme::ALL.iter().map(|s| s.name()).collect();
    assert_eq!(
        names,
        [
            "local",
            "archive",
            "remote",
            "shell",
            "collection",
            "search"
        ]
    );
    for &s in Scheme::ALL {
        assert_eq!(Scheme::from_name(s.name()), Some(s));
        assert_eq!(Scheme::from_name(&s.name().to_uppercase()), Some(s));
        assert_eq!(s.to_string(), s.name());
    }
    assert_eq!(Scheme::from_name("sftp"), None);
}

#[test]
fn local_rejects_empty_relative_and_nul() {
    assert_eq!(VPath::local(""), Err(PathError::Empty));
    assert_eq!(VPath::parse(""), Err(PathError::Empty));
    assert_eq!(
        VPath::local("rel/x"),
        Err(PathError::NotAbsolute("rel/x".into()))
    );
    assert_eq!(VPath::local(abs(&["a\0b"])), Err(PathError::Nul));
}

#[test]
fn local_normalizes_dot_segments() {
    let p = VPath::local(abs(&["a", ".", "b", "..", "c"])).unwrap();
    assert_eq!(p.scheme(), Scheme::Local);
    assert_eq!(p.as_local(), Some(abs(&["a", "c"]).as_path()));
    assert_eq!(local(&["..", ".."]).as_local(), Some(abs(&[]).as_path()));
}

#[test]
fn uri_roundtrip_escapes_percent_and_controls() {
    let p = local(&["dir", "100%.txt"]);
    assert!(p.to_uri().starts_with("local:"));
    assert!(p.to_uri().ends_with("100%25.txt"));
    assert_eq!(VPath::parse(&p.to_uri()), Ok(p.clone()));
    assert_eq!(VPath::parse(&p.to_string()), Ok(p.clone()));
    assert_eq!(p.to_string().parse::<VPath>(), Ok(p));
    let nl = local(&["a\nb"]);
    assert!(nl.to_uri().ends_with("a%0Ab"));
    assert_eq!(VPath::parse(&nl.to_uri()), Ok(nl));
}

#[test]
fn native_input_is_never_unescaped() {
    let p = VPath::parse(abs(&["a%41"]).to_str().unwrap()).unwrap();
    assert_eq!(
        p.as_local().and_then(Path::file_name),
        Some(OsStr::new("a%41"))
    );
    assert!(p.to_uri().ends_with("a%2541"));
}

#[test]
fn unicode_is_kept_exactly() {
    let nfc = local(&["한글"]);
    let nfd = local(&["\u{1112}\u{1161}\u{11AB}\u{1100}\u{1173}\u{11AF}"]);
    assert_ne!(nfc, nfd);
    assert!(nfc.to_uri().ends_with("한글"));
    assert_eq!(VPath::parse(&nfd.to_uri()), Ok(nfd));
    let emoji = local(&["🦝.png"]);
    assert_eq!(VPath::parse(&emoji.to_string()), Ok(emoji));
}

#[test]
fn parse_rejects_unknown_scheme_and_bad_escape() {
    assert_eq!(
        VPath::parse("sftp://host/x"),
        Err(PathError::UnknownScheme("sftp".into()))
    );
    assert_eq!(
        VPath::parse("local:%zz"),
        Err(PathError::InvalidEscape("%zz".into()))
    );
    assert_eq!(
        VPath::parse("local:relative"),
        Err(PathError::NotAbsolute("relative".into()))
    );
}

#[test]
fn unimplemented_scheme_is_unsupported() {
    assert!(matches!(
        VPath::parse("archive:/x"),
        Err(PathError::Unsupported(_))
    ));
}

#[test]
fn scheme_names_ignore_ascii_case() {
    let p = local(&["x"]);
    let uri = p.to_uri();
    assert!(uri.starts_with("local:"));
    for name in ["LOCAL", "Local"] {
        let spelled = uri.replacen("local", name, 1);
        assert_eq!(VPath::parse(&spelled), Ok(p.clone()), "{spelled}");
    }
}

#[cfg(unix)]
mod unix {
    use super::*;
    use std::os::unix::ffi::OsStrExt;

    #[test]
    fn non_utf8_bytes_roundtrip() {
        let raw = Path::new(OsStr::from_bytes(b"/tmp/\xff\xfe.bin"));
        let p = VPath::local(raw).unwrap();
        assert_eq!(p.to_uri(), "local:/tmp/%FF%FE.bin");
        assert_eq!(VPath::parse(&p.to_uri()), Ok(p.clone()));
        assert_eq!(p.as_local(), Some(raw));
        assert!(p.to_string().contains('\u{FFFD}'));
    }

    #[test]
    fn persisted_form_is_pinned() {
        let uri = |s: &str| VPath::local(s).unwrap().to_uri();
        assert_eq!(uri("/a/b"), "local:/a/b");
        assert_eq!(uri("/"), "local:/");
    }

    #[test]
    fn display_pads_even_with_invalid_bytes() {
        let p = VPath::local(Path::new(OsStr::from_bytes(b"/\xff"))).unwrap();
        assert_eq!(format!("{p:<4}|"), "/\u{FFFD}  |");
        assert_eq!(format!("{p:>4}|"), "  /\u{FFFD}|");
    }

    #[test]
    fn repeated_slashes_collapse() {
        assert_eq!(
            VPath::local("//a///b/").unwrap().as_local(),
            Some(Path::new("/a/b"))
        );
    }

    proptest::proptest! {
        #[test]
        fn any_byte_names_roundtrip(
            names in proptest::collection::vec(proptest::collection::vec(1u8..=255, 1..12), 0..5),
        ) {
            let mut raw = PathBuf::from("/");
            for n in names.iter().filter(|n| !n.contains(&b'/') && n.as_slice() != b"." && n.as_slice() != b"..") {
                raw.push(OsStr::from_bytes(n));
            }
            let p = VPath::local(&raw).unwrap();
            proptest::prop_assert_eq!(p.as_local(), Some(raw.as_path()));
            proptest::prop_assert_eq!(VPath::parse(&p.to_uri()), Ok(p));
        }
    }
}

#[cfg(windows)]
mod windows {
    use super::*;
    use std::ffi::OsString;
    use std::os::windows::ffi::{OsStrExt, OsStringExt};

    fn lp(s: &str) -> Option<PathBuf> {
        VPath::local(s)
            .ok()
            .and_then(|p| p.as_local().map(Path::to_path_buf))
    }

    #[test]
    fn drive_letters_uppercase_and_separators_unify() {
        assert_eq!(lp(r"c:\Users\..\x"), Some(PathBuf::from(r"C:\x")));
        assert_eq!(lp("C:/a/b"), Some(PathBuf::from(r"C:\a\b")));
    }

    #[test]
    fn verbatim_prefix_is_stripped() {
        assert_eq!(lp(r"\\?\C:\x"), Some(PathBuf::from(r"C:\x")));
        assert_eq!(
            lp(r"\\?\UNC\srv\share\x"),
            Some(PathBuf::from(r"\\srv\share\x"))
        );
    }

    #[test]
    fn rejects_relative_device_and_stream_forms() {
        assert_eq!(
            VPath::local(r"C:x"),
            Err(PathError::NotAbsolute("C:x".into()))
        );
        assert_eq!(
            VPath::local(r"\x"),
            Err(PathError::NotAbsolute(r"\x".into()))
        );
        assert_eq!(
            VPath::local(r"\\.\COM1"),
            Err(PathError::Unsupported(r"\\.\COM1".into()))
        );
        assert_eq!(
            VPath::local(r"C:\a\b:c"),
            Err(PathError::InvalidName("b:c".into()))
        );
    }

    #[test]
    fn persisted_form_is_pinned() {
        let uri = |s: &str| VPath::local(s).unwrap().to_uri();
        assert_eq!(uri(r"C:\"), r"local:C:\");
        assert_eq!(uri(r"C:\a\b"), r"local:C:\a\b");
        assert_eq!(uri(r"\\srv\share"), r"local:\\srv\share\");
        assert_eq!(uri(r"\\srv\share\x"), r"local:\\srv\share\x");
    }

    /// With `/` in the prefix, std reads `//?/C:/x` as a share `C:` on a server named `?`
    /// (and `//./x` as one on `.`); stored that way it would not survive `to_uri`/`parse`.
    #[test]
    fn mixed_separator_verbatim_and_device_forms_are_rejected() {
        for input in [
            "//?/C:/x",
            r"\\?/C:\x",
            r"/\?\C:\x",
            "//?/UNC/srv/share/x",
            "//./C:/x",
            r"\\./C:\x",
        ] {
            assert_eq!(
                VPath::local(input),
                Err(PathError::Unsupported(input.into())),
                "{input}"
            );
        }
        assert_eq!(lp(r"\\?x\share"), Some(PathBuf::from(r"\\?x\share\")));
    }

    #[test]
    fn unc_share_root_needs_no_trailing_separator() {
        let root = Some(PathBuf::from(r"\\srv\share\"));
        assert_eq!(lp(r"\\srv\share"), root);
        assert_eq!(lp(r"\\srv\share\"), root);
        assert_eq!(lp(r"\\?\UNC\srv\share"), root);
        assert_eq!(lp(r"\\srv\share\.."), root);
        assert_eq!(lp(r"\\srv"), None);
    }

    #[test]
    fn lone_surrogate_roundtrips() {
        let mut w: Vec<u16> = r"C:\".encode_utf16().collect();
        w.push(0xD800);
        let p = VPath::local(PathBuf::from(OsString::from_wide(&w))).unwrap();
        assert_eq!(p.to_uri(), r"local:C:\%ED%A0%80");
        assert_eq!(VPath::parse(&p.to_uri()), Ok(p.clone()));
        let back: Vec<u16> = p.as_local().unwrap().as_os_str().encode_wide().collect();
        assert_eq!(back, w);
    }

    proptest::proptest! {
        #[test]
        fn any_unit_names_roundtrip(
            names in proptest::collection::vec(proptest::collection::vec(1u16..=u16::MAX, 1..12), 0..5),
        ) {
            let bad = |u: &u16| [u16::from(b'/'), u16::from(b'\\'), u16::from(b':')].contains(u);
            let mut raw = PathBuf::from(r"C:\");
            for n in names.iter().filter(|n| !n.iter().any(bad) && n.iter().any(|u| *u != u16::from(b'.'))) {
                raw.push(OsString::from_wide(n));
            }
            let p = VPath::local(&raw).unwrap();
            proptest::prop_assert_eq!(p.as_local(), Some(raw.as_path()));
            proptest::prop_assert_eq!(VPath::parse(&p.to_uri()), Ok(p));
        }
    }
}
