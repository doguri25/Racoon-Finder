use std::path::{Path, PathBuf};

use rf_core::identity;

#[test]
fn identity_values() {
    assert_eq!(identity::DISPLAY_NAME, "Racoon Finder");
    assert_eq!(identity::DATA_DIR_NAME, "RacoonFinder");
    assert_eq!(identity::BUNDLE_ID, "io.github.doguri25.RacoonFinder");
    assert_eq!(identity::URL_SCHEME, "racoonfinder");
    assert_eq!(identity::TEMP_PREFIX, "RacoonFinder-");
    assert_eq!(identity::PORTABLE_MARKER, "portable");
    assert_eq!(identity::PORTABLE_DATA_DIR, "data");
    assert_eq!(identity::VERSION, env!("CARGO_PKG_VERSION"));
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd {
        let p = e.unwrap().path();
        if p.file_name().is_some_and(|n| n == "target") {
            continue;
        }
        if p.is_dir() {
            rust_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// PRD Q1 may rename the product: only `identity.rs` may spell the name.
#[test]
fn product_name_appears_only_in_identity_module() {
    let core = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace = core.parent().unwrap().parent().unwrap();
    let allowed = [
        core.join("src").join("identity.rs"),
        core.join("tests").join("identity.rs"),
    ];
    let needle = identity::DISPLAY_NAME
        .split(' ')
        .next()
        .unwrap()
        .to_lowercase();
    let mut files = Vec::new();
    rust_files(&workspace.join("crates"), &mut files);
    rust_files(&workspace.join("apps"), &mut files);
    assert!(
        files.contains(&allowed[0]),
        "scan did not reach identity.rs: {files:?}"
    );
    let offenders: Vec<_> = files
        .iter()
        .filter(|f| !allowed.contains(f))
        .filter(|f| {
            std::fs::read_to_string(f)
                .unwrap()
                .to_lowercase()
                .contains(&needle)
        })
        .collect();
    assert!(
        offenders.is_empty(),
        "product name outside identity.rs: {offenders:?}"
    );
}
