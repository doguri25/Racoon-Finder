//! Compiles the Slint screens under `ui/` into Rust code included by `src/lib.rs`, and lists the
//! properties of the `Tr` global (`ui/tr.slint`, the window's words) for `src/ui_bridge.rs`,
//! which fills them.

use std::fmt::Write as _;

fn main() {
    // Debug builds carry the element names that the headless tests use to find a button or a
    // row (`i_slint_backend_testing::ElementHandle`); release builds leave them out.
    let debug = std::env::var("PROFILE").is_ok_and(|profile| profile == "debug");
    let config = slint_build::CompilerConfiguration::new()
        .with_style("fluent".into())
        .with_debug_info(debug);
    slint_build::compile_with_config("ui/app.slint", config).expect("compile ui/app.slint");
    list_words();
}

/// Writes `tr_properties.rs` to the build's output: an array of every property of `Tr`, in the
/// order of `ui/tr.slint`, each as its key (the property's name, which is the key of its message
/// in `ui.ftl`), a function that sets it and one that reads it. The file holds nothing but
/// `in property <string> name;` lines, comments and the global's braces; anything else stops the
/// build, so that no property can be missed here.
fn list_words() {
    const FILE: &str = "ui/tr.slint";
    println!("cargo:rerun-if-changed={FILE}");
    let source = std::fs::read_to_string(FILE).expect("read ui/tr.slint");
    let mut names: Vec<&str> = Vec::new();
    let mut opened = false;
    let mut closed = false;
    for (number, line) in source.lines().enumerate() {
        let line = line.trim();
        let wrong = |what: &str| -> ! { panic!("{FILE}:{}: {what}: {line}", number + 1) };
        if line.is_empty() || line.starts_with("//") {
            continue;
        }
        if line == "export global Tr {" && !opened {
            opened = true;
        } else if line == "}" && opened && !closed {
            closed = true;
        } else if let Some(name) = line
            .strip_prefix("in property <string> ")
            .and_then(|rest| rest.strip_suffix(';'))
        {
            if !opened || closed {
                wrong("a property outside `export global Tr { … }`");
            }
            let well_formed = name.starts_with(|c: char| c.is_ascii_lowercase())
                && !name.ends_with('-')
                && !name.contains("--")
                && name
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
            if !well_formed {
                wrong("a name that is not a key (lowercase words joined by `-`)");
            }
            if names.contains(&name) {
                wrong("a property twice");
            }
            names.push(name);
        } else {
            wrong("only `in property <string> name;` lines (no value) and comments");
        }
    }
    assert!(opened && closed, "{FILE}: no `export global Tr {{ … }}`");
    let mut out = String::from(
        "// Made by build.rs from ui/tr.slint: each property of `Tr`, its key, its setter and its getter.\n[\n",
    );
    for name in names {
        let rust = name.replace('-', "_");
        writeln!(
            out,
            "    (\"{name}\", |tr: &Tr<'_>, text: SharedString| tr.set_{rust}(text), |tr: &Tr<'_>| tr.get_{rust}()),"
        )
        .expect("write to a string");
    }
    out.push_str("]\n");
    let dir = std::env::var("OUT_DIR").expect("OUT_DIR");
    std::fs::write(std::path::Path::new(&dir).join("tr_properties.rs"), out)
        .expect("write tr_properties.rs");
}
