// Reads `[package.metadata.hotdata]` from Cargo.toml and re-exports the
// values as compile-time environment variables, so source files can read
// distribution config (e.g. the Homebrew formula) via `env!()` without
// duplicating strings that also live in README.md / dist-workspace.toml.

use std::path::PathBuf;

fn main() {
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let manifest_path = manifest_dir.join("Cargo.toml");
    println!("cargo:rerun-if-changed={}", manifest_path.display());

    let raw = std::fs::read_to_string(&manifest_path)
        .unwrap_or_else(|e| panic!("could not read {}: {e}", manifest_path.display()));
    let parsed: toml::Value = toml::from_str(&raw)
        .unwrap_or_else(|e| panic!("could not parse {}: {e}", manifest_path.display()));

    let meta = parsed
        .get("package")
        .and_then(|p| p.get("metadata"))
        .and_then(|m| m.get("hotdata"))
        .unwrap_or_else(|| panic!("missing [package.metadata.hotdata] in Cargo.toml"));

    let homebrew_formula = meta
        .get("homebrew_formula")
        .and_then(|v| v.as_str())
        .unwrap_or_else(|| panic!("missing package.metadata.hotdata.homebrew_formula"));

    println!("cargo:rustc-env=HOTDATA_HOMEBREW_FORMULA={homebrew_formula}");
}
