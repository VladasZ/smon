//! Downloading a released binary and proving it arrived intact.

use std::{
    env::consts::{ARCH, OS},
    fs::File,
    io::{Cursor, IsTerminal, Read, copy, stderr},
    path::Path,
};

use anyhow::{Context, Result, bail};
use flate2::read::GzDecoder;
use hex::encode;
use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};
use reqwest::blocking::Client;
use sha2::{Digest, Sha256};
use tar::Archive;
use zip::ZipArchive;

use super::{install::BINARY, release::REPOSITORY};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Format {
    TarGz,
    Zip,
}

impl Format {
    const fn extension(self) -> &'static str {
        match self {
            Self::TarGz => "tar.gz",
            Self::Zip => "zip",
        }
    }
}

pub struct Asset {
    pub target: &'static str,
    pub format: Format,
    pub name:   String,
}

/// The prebuilt asset for the host, or `None` on a platform the release
/// workflow does not build.
pub fn for_host(tag: &str) -> Option<Asset> {
    let (target, format) = match (OS, ARCH) {
        ("linux", "x86_64") => ("x86_64-unknown-linux-musl", Format::TarGz),
        ("linux", "aarch64") => ("aarch64-unknown-linux-musl", Format::TarGz),
        ("macos", _) => ("universal-apple-darwin", Format::TarGz),
        ("windows", "x86_64") => ("x86_64-pc-windows-msvc", Format::Zip),
        ("windows", "aarch64") => ("aarch64-pc-windows-msvc", Format::Zip),
        _ => return None,
    };
    Some(Asset {
        target,
        format,
        name: asset_name(tag, target, format),
    })
}

fn asset_name(tag: &str, target: &str, format: Format) -> String {
    format!("smon-{tag}-{target}.{}", format.extension())
}

pub fn download_url(tag: &str, file: &str) -> String {
    format!("{REPOSITORY}/releases/download/{tag}/{file}")
}

fn client() -> Result<Client> {
    Client::builder()
        .user_agent(concat!("smon/", env!("CARGO_PKG_VERSION")))
        .build()
        .context("could not build an http client")
}

/// # Errors
/// Returns an error if the request fails or the response is not text.
pub fn fetch_text(url: &str) -> Result<String> {
    client()?
        .get(url)
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .with_context(|| format!("could not download {url}"))?
        .text()
        .with_context(|| format!("could not read {url}"))
}

/// # Errors
/// Returns an error if the request fails or the body cannot be read.
pub fn download(url: &str) -> Result<Vec<u8>> {
    let response = client()?
        .get(url)
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .with_context(|| format!("could not download {url}"))?;

    let total = response.content_length().unwrap_or(0);
    let bar = ProgressBar::new(total);
    bar.set_style(
        ProgressStyle::with_template("  [{bar:40}] {bytes}/{total_bytes} {bytes_per_sec} {eta}")?
            .progress_chars("=>-"),
    );
    if !stderr().is_terminal() {
        bar.set_draw_target(ProgressDrawTarget::hidden());
    }

    let mut bytes = Vec::with_capacity(total as usize);
    bar.wrap_read(response)
        .read_to_end(&mut bytes)
        .with_context(|| format!("could not download {url}"))?;
    bar.finish_and_clear();
    Ok(bytes)
}

/// A tampered or truncated download must fail here, not surface later as a
/// binary that will not start.
///
/// # Errors
/// Returns an error if the asset is unlisted or its hash does not match.
pub fn verify_checksum(sums: &str, asset: &str, bytes: &[u8]) -> Result<()> {
    let expected = sums
        .lines()
        .find_map(|line| {
            let mut parts = line.split_whitespace();
            let hash = parts.next()?;
            // sha256sum marks a binary read with a star before the name.
            let name = parts.next()?.trim_start_matches('*');
            (name == asset).then(|| hash.to_ascii_lowercase())
        })
        .with_context(|| format!("{asset} is not listed in SHA256SUMS"))?;

    let actual = encode(Sha256::digest(bytes));
    if actual != expected {
        bail!("{asset} failed its checksum, expected {expected}, got {actual}");
    }
    Ok(())
}

/// Pull the `smon` binary out of the archive and write it to `dest`.
///
/// # Errors
/// Returns an error if the archive is unreadable, holds no binary, or the
/// destination cannot be written.
pub fn extract(bytes: &[u8], format: Format, dest: &Path) -> Result<()> {
    match format {
        Format::TarGz => extract_tar_gz(bytes, dest),
        Format::Zip => extract_zip(bytes, dest),
    }?;
    make_executable(dest)
}

fn extract_tar_gz(bytes: &[u8], dest: &Path) -> Result<()> {
    let mut archive = Archive::new(GzDecoder::new(Cursor::new(bytes)));
    for entry in archive.entries().context("could not read the archive")? {
        let mut entry = entry.context("could not read an archive entry")?;
        let path = entry.path().context("an archive entry has no path")?;
        if !is_binary(&path.to_string_lossy()) {
            continue;
        }
        let mut file =
            File::create(dest).with_context(|| format!("could not write {}", dest.display()))?;
        copy(&mut entry, &mut file)
            .with_context(|| format!("could not write {}", dest.display()))?;
        return Ok(());
    }
    bail!("the archive does not contain a {BINARY} binary")
}

fn extract_zip(bytes: &[u8], dest: &Path) -> Result<()> {
    let mut archive = ZipArchive::new(Cursor::new(bytes)).context("could not read the archive")?;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .context("could not read an archive entry")?;
        if !is_binary(entry.name()) {
            continue;
        }
        let mut file =
            File::create(dest).with_context(|| format!("could not write {}", dest.display()))?;
        copy(&mut entry, &mut file)
            .with_context(|| format!("could not write {}", dest.display()))?;
        return Ok(());
    }
    bail!("the archive does not contain a {BINARY} binary")
}

fn is_binary(path: &str) -> bool {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    name == "smon" || name == "smon.exe"
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<()> {
    use std::{
        fs::{Permissions, set_permissions},
        os::unix::fs::PermissionsExt,
    };

    set_permissions(path, Permissions::from_mode(0o755))
        .with_context(|| format!("could not make {} executable", path.display()))
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{
        env::temp_dir,
        fs::{create_dir_all, read_to_string, remove_dir_all},
        io::Write,
    };

    use flate2::{Compression, write::GzEncoder};
    use tar::{Builder, Header};

    use super::{Format, asset_name, download_url, extract, is_binary, verify_checksum};

    const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    #[test]
    fn asset_names_match_what_the_release_workflow_packages() {
        assert_eq!(
            asset_name("v0.1.2", "x86_64-unknown-linux-musl", Format::TarGz),
            "smon-v0.1.2-x86_64-unknown-linux-musl.tar.gz"
        );
        assert_eq!(
            asset_name("v0.1.2", "x86_64-pc-windows-msvc", Format::Zip),
            "smon-v0.1.2-x86_64-pc-windows-msvc.zip"
        );
    }

    #[test]
    fn download_urls_point_at_the_release_assets() {
        assert_eq!(
            download_url("v0.1.2", "SHA256SUMS"),
            "https://github.com/VladasZ/smon/releases/download/v0.1.2/SHA256SUMS"
        );
    }

    #[test]
    fn a_matching_checksum_passes() {
        let sums = format!("{EMPTY_SHA256}  smon-v0.1.2-x86_64-unknown-linux-musl.tar.gz\n");
        verify_checksum(&sums, "smon-v0.1.2-x86_64-unknown-linux-musl.tar.gz", b"").unwrap();
    }

    #[test]
    fn a_wrong_checksum_fails() {
        let sums = format!("{EMPTY_SHA256}  asset.tar.gz\n");
        let error = verify_checksum(&sums, "asset.tar.gz", b"tampered").unwrap_err();
        assert!(error.to_string().contains("failed its checksum"));
    }

    #[test]
    fn an_asset_missing_from_the_sums_fails() {
        let sums = format!("{EMPTY_SHA256}  other.tar.gz\n");
        let error = verify_checksum(&sums, "asset.tar.gz", b"").unwrap_err();
        assert!(error.to_string().contains("is not listed in SHA256SUMS"));
    }

    #[test]
    fn only_the_named_binary_is_extracted() {
        assert!(is_binary("smon"));
        assert!(is_binary("smon.exe"));
        assert!(is_binary("./smon"));
        assert!(is_binary("dist\\smon.exe"));
        assert!(!is_binary("README.md"));
        assert!(!is_binary("smonitor"));
    }

    #[test]
    fn the_binary_comes_out_of_a_tar_gz() {
        let mut tar = Builder::new(Vec::new());
        let mut header = Header::new_gnu();
        header.set_size(5);
        header.set_mode(0o755);
        header.set_cksum();
        tar.append_data(&mut header, "smon", &b"hello"[..]).unwrap();
        let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
        encoder.write_all(&tar.into_inner().unwrap()).unwrap();
        let archive = encoder.finish().unwrap();

        let dir = temp_dir().join("smon-extract-test");
        if dir.exists() {
            remove_dir_all(&dir).unwrap();
        }
        create_dir_all(&dir).unwrap();
        let dest = dir.join("smon");
        extract(&archive, Format::TarGz, &dest).unwrap();
        assert_eq!(read_to_string(&dest).unwrap(), "hello");
        remove_dir_all(&dir).unwrap();
    }
}
