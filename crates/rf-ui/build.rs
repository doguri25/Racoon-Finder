//! Compiles the Slint screens under `ui/` into Rust code included by `src/lib.rs`.

fn main() {
    let config = slint_build::CompilerConfiguration::new().with_style("fluent".into());
    slint_build::compile_with_config("ui/app.slint", config).expect("compile ui/app.slint");
}
