//! ozmux developer tasks. Provides `bundle-ozmux-daemon`, which assembles
//! `target/<profile>/ozmux-daemon.app` (and helper sub-bundles) so multi-process CEF
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
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(name = "xtask", about = "ozmux developer tasks", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Assemble `target/<profile>/ozmux-daemon.app` from pre-built `ozmux-daemon`
    /// and `cef_helper` binaries. macOS-only; on other platforms this prints a
    /// notice and exits successfully. Injects `LSUIElement=YES` into the Info.plist
    /// so the daemon does not appear in the Dock.
    BundleOzmuxDaemon {
        /// Bundle from `target/release/` instead of `target/debug/`.
        #[arg(long)]
        release: bool,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::BundleOzmuxDaemon { release } => bundle_ozmux_daemon(release),
    }
}

#[cfg(not(target_os = "macos"))]
fn bundle_ozmux_daemon(_release: bool) -> Result<()> {
    println!("xtask bundle-ozmux-daemon: only macOS is supported");
    Ok(())
}

#[cfg(target_os = "macos")]
fn workspace_root() -> PathBuf {
    // NOTE: CARGO_MANIFEST_DIR is `<root>/xtask`; parent is the workspace root.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask manifest dir has a parent")
        .to_path_buf()
}

#[cfg(target_os = "macos")]
fn bundle_ozmux_daemon(release: bool) -> Result<()> {
    let workspace_root = workspace_root();
    let profile = if release { "release" } else { "debug" };
    let target_dir = workspace_root.join("target").join(profile);

    let daemon_bin = target_dir.join("ozmux-daemon");
    let cef_helper_bin = target_dir.join("cef_helper");
    if !daemon_bin.exists() {
        bail!(
            "ozmux-daemon binary not found at {}; run `cargo build -p daemon_bootstrap --bin ozmux-daemon` first",
            daemon_bin.display()
        );
    }
    if !cef_helper_bin.exists() {
        bail!(
            "cef_helper binary not found at {}; run `cargo build -p ozmux_cef_host --bin cef_helper` first",
            cef_helper_bin.display()
        );
    }

    let info = BundleInfo::new(
        "ozmux-daemon",
        "com.ozmux.daemon",
        "ozmux daemon",
        "English",
        Version::parse("0.1.0").expect("static version string parses"),
    );

    let app_path = bundle(
        &target_dir,
        &target_dir,
        "ozmux-daemon",
        "cef_helper",
        None,
        info,
    )
    .context("cef::build_util::mac::bundle failed")?;

    let info_plist = app_path.join("Contents").join("Info.plist");
    inject_lsuielement(&info_plist)
        .with_context(|| format!("inject LSUIElement=YES into {}", info_plist.display()))?;

    println!("bundled {}", app_path.display());
    Ok(())
}

/// Inject `LSUIElement=YES` into the Info.plist via PlistBuddy so the daemon
/// runs as an accessory app (no Dock icon, no app-switcher entry). Uses `Set`
/// when the key already exists and falls back to `Add` otherwise.
#[cfg(target_os = "macos")]
fn inject_lsuielement(info_plist: &Path) -> Result<()> {
    let set_status = std::process::Command::new("/usr/libexec/PlistBuddy")
        .args(["-c", "Set :LSUIElement YES"])
        .arg(info_plist)
        .status()
        .context("spawn PlistBuddy Set")?;
    if set_status.success() {
        return Ok(());
    }
    let add_status = std::process::Command::new("/usr/libexec/PlistBuddy")
        .args(["-c", "Add :LSUIElement bool YES"])
        .arg(info_plist)
        .status()
        .context("spawn PlistBuddy Add")?;
    if !add_status.success() {
        bail!(
            "PlistBuddy Add :LSUIElement failed for {}",
            info_plist.display()
        );
    }
    Ok(())
}
