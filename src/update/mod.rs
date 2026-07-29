//! `smon update`: replace the installed binary with a released one, then get
//! every running smon off the old version.

mod asset;
mod install;
mod records;
mod release;
mod running;

use std::{
    env::consts::{ARCH, OS},
    fs::remove_file,
    net::SocketAddr,
    path::Path,
    process::Command,
};

use anyhow::{Context, Result, bail};

use asset::{Asset, download, download_url, extract, fetch_text, verify_checksum};
use install::{
    BINARY, cargo_home, cleanup_stale_binaries, move_aside, restore, swap, verify, warn_if_shadowed,
};
use records::{install_key, record};
use release::{
    INSTALLED, PACKAGE, Release, exact, fetch_tags, format_version, installed_version, newest,
};

#[derive(Debug, Default)]
pub struct Request {
    pub version:     Option<String>,
    pub from_source: bool,
    pub yes:         bool,
}

/// # Errors
/// Returns an error if the release cannot be found, the download fails its
/// checksum or version check, or the binary cannot be put in place.
pub fn update(request: &Request, mcp: SocketAddr) -> Result<()> {
    let home = cargo_home()?;
    let target = home.join("bin").join(BINARY);
    if cfg!(windows) {
        cleanup_stale_binaries(&target);
    }

    let tags = fetch_tags()?;
    let release = match &request.version {
        Some(wanted) => {
            exact(&tags, wanted).with_context(|| format!("smon has no release tagged {wanted}"))?
        }
        None => newest(&tags).context("the smon repository has no released version yet")?,
    };

    // An asked for version always installs, so a downgrade or a repair of a
    // broken binary both work.
    if request.version.is_none()
        && let (Some(installed), Some(latest)) = (installed_version(), release.version)
        && installed >= latest
    {
        println!("smon is already up to date (v{})", format_version(installed));
        return Ok(());
    }

    // Asked before anything is downloaded, so a no costs nothing, and read
    // before the swap, because afterwards a process reports a path whose file
    // has already been renamed away.
    let live = running::find(mcp);
    running::confirm(&live, request.yes)?;

    println!("updating smon from v{INSTALLED} to {}", release.tag);

    match asset_for(&release, request.from_source) {
        Some(asset) => install_asset(&release, &asset, &home, &target)?,
        None => install_from_source(&release.tag, &target)?,
    }

    println!("updated smon to {}", release.tag);
    warn_if_shadowed(&target);
    running::restart(&live, &target);
    Ok(())
}

fn asset_for(release: &Release, from_source: bool) -> Option<Asset> {
    if from_source {
        return None;
    }
    let asset = asset::for_host(&release.tag);
    if asset.is_none() {
        eprintln!("warning: there is no prebuilt smon binary for {OS} {ARCH}");
        eprintln!("warning: building {} from source instead", release.tag);
    }
    asset
}

/// Download, prove the binary works, and only then touch the installed one.
fn install_asset(release: &Release, asset: &Asset, home: &Path, target: &Path) -> Result<()> {
    println!("downloading {}", asset.name);
    let archive = download(&download_url(&release.tag, &asset.name))?;
    let sums = fetch_text(&download_url(&release.tag, "SHA256SUMS"))?;
    verify_checksum(&sums, &asset.name, &archive)?;

    // Staged next to the target so the swap is a rename inside one filesystem.
    let staged = target.with_file_name(if cfg!(windows) {
        "smon.new.exe"
    } else {
        "smon.new"
    });
    extract(&archive, asset.format, &staged)?;

    if let Err(error) = verify(&staged, &release.tag) {
        discard(&staged);
        return Err(error);
    }
    swap(&staged, target)?;

    if let Err(error) = record(home, &install_key(&release.tag), asset.target) {
        eprintln!(
            "warning: smon is installed but cargo's install list was not updated: {error:#}"
        );
    }
    Ok(())
}

fn install_from_source(tag: &str, target: &Path) -> Result<()> {
    // Windows cannot overwrite the running binary, so cargo needs it out of the
    // way. Cargo writes its own install records on this path.
    let moved = if cfg!(windows) && target.exists() {
        Some(move_aside(target)?)
    } else {
        None
    };

    let outcome = run_cargo_install(tag).and_then(|()| {
        if target.exists() {
            Ok(())
        } else {
            bail!(
                "cargo reported success but did not install {}",
                target.display()
            )
        }
    });

    let Some(old) = moved else {
        return outcome;
    };
    match outcome {
        Ok(()) => {
            discard(&old);
            Ok(())
        }
        Err(error) => {
            if let Err(restore_error) = restore(&old, target) {
                bail!(
                    "{error:#}; restoring the previous smon binary also failed: {restore_error:#}"
                );
            }
            Err(error)
        }
    }
}

fn run_cargo_install(tag: &str) -> Result<()> {
    let version = tag.strip_prefix('v').unwrap_or(tag);
    let status = Command::new("cargo")
        .args(["install", PACKAGE, "--version", version])
        .status()
        .context("could not start cargo install")?;
    if !status.success() {
        bail!("cargo install exited with {status}");
    }
    Ok(())
}

fn discard(path: &Path) {
    if let Err(error) = remove_file(path) {
        eprintln!("smon update: could not remove {}: {error}", path.display());
    }
}
