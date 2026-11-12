//! Compiles the Slint screens under `ui/` into Rust code included by `src/lib.rs`.

fn main() {
    // Debug builds carry the element names that the headless tests use to find a button or a
    // row (`i_slint_backend_testing::ElementHandle`); release builds leave them out.
    let debug = std::env::var("PROFILE").is_ok_and(|profile| profile == "debug");
    let config = slint_build::CompilerConfiguration::new()
        .with_style("fluent".into())
        .with_debug_info(debug);
    slint_build::compile_with_config("ui/app.slint", config).expect("compile ui/app.slint");
}
