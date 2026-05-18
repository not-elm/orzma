//! ozmux developer tasks. Currently provides `bundle-cef-host`, which assembles
//! `target/<profile>/cef_host.app` (and helper sub-bundles) so multi-process CEF
//! can perform Mach port rendezvous on macOS.

use anyhow::Result;
#[cfg(target_os = "macos")]
use anyhow::{Context as _, bail};
#[cfg(target_os = "macos")]
use cef::build_util::mac::{BundleInfo, bundle};
use clap::{Parser, Subcommand};
#[cfg(target_os = "macos")]
use semver::Version;
#[cfg(target_os = "macos")]
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "xtask", about = "ozmux developer tasks", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Assemble `target/<profile>/cef_host.app` from pre-built `cef_host` and
    /// `cef_helper` binaries. macOS-only; on other platforms this prints a
    /// notice and exits successfully.
    BundleCefHost {
        /// Bundle from `target/release/` instead of `target/debug/`.
        #[arg(long)]
        release: bool,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::BundleCefHost { release } => bundle_cef_host(release),
    }
}

#[cfg(not(target_os = "macos"))]
fn bundle_cef_host(_release: bool) -> Result<()> {
    println!("xtask bundle-cef-host: only macOS is supported");
    Ok(())
}

#[cfg(target_os = "macos")]
fn bundle_cef_host(release: bool) -> Result<()> {
    // NOTE: CARGO_MANIFEST_DIR is `<root>/xtask`; parent is the workspace root.
    let workspace_root: PathBuf = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask manifest dir has a parent")
        .to_path_buf();
    let profile = if release { "release" } else { "debug" };
    let target_dir = workspace_root.join("target").join(profile);

    let cef_host_bin = target_dir.join("cef_host");
    let cef_helper_bin = target_dir.join("cef_helper");
    if !cef_host_bin.exists() {
        bail!(
            "cef_host binary not found at {}; run `cargo build -p ozmux_cef_host` first",
            cef_host_bin.display()
        );
    }
    if !cef_helper_bin.exists() {
        bail!(
            "cef_helper binary not found at {}; run `cargo build -p ozmux_cef_host` first",
            cef_helper_bin.display()
        );
    }

    let info = BundleInfo::new(
        "cef_host",
        "com.ozmux.cef-host",
        "ozmux cef_host",
        "English",
        Version::parse("0.1.0").expect("static version string parses"),
    );

    let app_path = bundle(
        &target_dir,
        &target_dir,
        "cef_host",
        "cef_helper",
        None,
        info,
    )
    .context("cef::build_util::mac::bundle failed")?;

    println!("bundled {}", app_path.display());
    Ok(())
}
