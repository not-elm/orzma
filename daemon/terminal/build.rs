//! Emits `ALACRITTY_TERMINAL_VERSION` env var by parsing the workspace
//! `Cargo.lock` at build time. `Tape::load` uses
//! `option_env!("ALACRITTY_TERMINAL_VERSION")` to verify tape captures
//! match the linked alacritty_terminal version.
use std::path::PathBuf;

fn main() {
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let mut p = manifest_dir.clone();
    let lock = loop {
        let candidate = p.join("Cargo.lock");
        if candidate.exists() {
            break candidate;
        }
        if !p.pop() {
            panic!("Cargo.lock not found walking up from {:?}", manifest_dir);
        }
    };
    println!("cargo:rerun-if-changed={}", lock.display());

    let lock_text = std::fs::read_to_string(&lock).unwrap();
    let parsed: toml::Value = toml::from_str(&lock_text).unwrap();
    let packages = parsed["package"].as_array().expect("Cargo.lock missing [package]");
    let version = packages
        .iter()
        .find(|p| p["name"].as_str() == Some("alacritty_terminal"))
        .and_then(|p| p["version"].as_str())
        .expect("alacritty_terminal not found in Cargo.lock");

    println!("cargo:rustc-env=ALACRITTY_TERMINAL_VERSION={}", version);
}
