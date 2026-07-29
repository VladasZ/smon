//! Which versions exist, and which one an update is heading for.

use std::{collections::HashSet, process::Command};

use anyhow::{Context, Result, bail};

pub const REPOSITORY: &str = "https://github.com/VladasZ/smon";
pub const PACKAGE: &str = "smon";
pub const INSTALLED: &str = env!("CARGO_PKG_VERSION");

pub type Version = (u64, u64, u64);

pub struct Release {
    pub tag:     String,
    /// `None` for a prerelease, which is only ever reached by asking for it.
    pub version: Option<Version>,
}

pub fn installed_version() -> Option<Version> {
    parse_version(INSTALLED)
}

pub fn format_version((major, minor, patch): Version) -> String {
    format!("{major}.{minor}.{patch}")
}

pub fn parse_version(tag: &str) -> Option<Version> {
    let tag = tag.strip_prefix('v').unwrap_or(tag);
    // A prerelease is never an update target, and a two component tag has no
    // patch, so both fail this parse on purpose.
    if tag.contains('-') {
        return None;
    }
    let mut parts = tag.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

/// # Errors
/// Returns an error if git is missing or the repository cannot be reached.
pub fn fetch_tags() -> Result<String> {
    let output = Command::new("git")
        .args(["ls-remote", "--tags", REPOSITORY])
        .output()
        .context("could not query the smon repository")?;
    if !output.status.success() {
        bail!("git ls-remote exited with {}", output.status);
    }
    String::from_utf8(output.stdout).context("git returned non-UTF-8 tags")
}

/// Every tag name the repository has. An annotated tag is listed twice, once
/// under its own name and once peeled with a `^{}` suffix, and both name the
/// same release, so the suffix is simply dropped.
fn tag_names(ls_remote: &str) -> HashSet<String> {
    ls_remote
        .lines()
        .filter_map(|line| {
            let reference = line.split_whitespace().nth(1)?;
            let tag = reference.strip_prefix("refs/tags/")?;
            Some(tag.strip_suffix("^{}").unwrap_or(tag).to_string())
        })
        .collect()
}

pub fn newest(ls_remote: &str) -> Option<Release> {
    tag_names(ls_remote)
        .into_iter()
        .filter_map(|tag| {
            Some(Release {
                version: Some(parse_version(&tag)?),
                tag,
            })
        })
        .max_by_key(|release| release.version)
}

/// The release for an exact tag, with or without the `v` the user typed.
pub fn exact(ls_remote: &str, wanted: &str) -> Option<Release> {
    let tags = tag_names(ls_remote);
    let prefixed = format!("v{wanted}");
    [wanted, prefixed.as_str()]
        .into_iter()
        .find(|name| tags.contains(*name))
        .map(|name| Release {
            tag:     name.to_string(),
            version: parse_version(name),
        })
}

#[cfg(test)]
mod tests {
    use super::{exact, newest, parse_version};

    const LS_REMOTE: &str = r"1111111111111111111111111111111111111111	refs/tags/v0.1.0
2222222222222222222222222222222222222222	refs/tags/v0.2.0
aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa	refs/tags/v0.2.0^{}
3333333333333333333333333333333333333333	refs/tags/v0.2
4444444444444444444444444444444444444444	refs/tags/v0.3.0-rc.1
5555555555555555555555555555555555555555	refs/tags/v0.10.0
";

    #[test]
    fn versions_parse_with_and_without_the_v_prefix() {
        assert_eq!(parse_version("v0.2.1"), Some((0, 2, 1)));
        assert_eq!(parse_version("0.2.1"), Some((0, 2, 1)));
        assert_eq!(parse_version("v10.20.30"), Some((10, 20, 30)));
    }

    // smon publishes no moving tag, but a stray one must never be mistaken for
    // a release to install.
    #[test]
    fn moving_tags_and_prereleases_are_not_update_targets() {
        assert_eq!(parse_version("v0.2"), None);
        assert_eq!(parse_version("v0.2.0-rc.1"), None);
        assert_eq!(parse_version("v0.2.0.1"), None);
        assert_eq!(parse_version("main"), None);
        assert_eq!(parse_version("vX.Y.Z"), None);
    }

    #[test]
    fn the_newest_release_wins_over_peeled_and_moving_tags() {
        let newest = newest(LS_REMOTE).unwrap();
        assert_eq!(newest.tag, "v0.10.0");
        assert_eq!(newest.version, Some((0, 10, 0)));
    }

    #[test]
    fn a_repository_without_releases_has_no_newest_release() {
        let ls_remote = "3333333333333333333333333333333333333333	refs/tags/v0.2\n";
        assert!(newest(ls_remote).is_none());
        assert!(newest("").is_none());
    }

    #[test]
    fn an_exact_tag_is_found_with_or_without_the_v() {
        assert_eq!(exact(LS_REMOTE, "0.1.0").unwrap().tag, "v0.1.0");
        assert_eq!(exact(LS_REMOTE, "v0.1.0").unwrap().tag, "v0.1.0");
        assert!(exact(LS_REMOTE, "v9.9.9").is_none());
    }

    // An annotated tag appears twice in ls-remote. Both lines are the same
    // release, so asking for it must not depend on which line was seen.
    #[test]
    fn an_annotated_tag_is_found_once() {
        assert_eq!(exact(LS_REMOTE, "v0.2.0").unwrap().tag, "v0.2.0");
    }

    #[test]
    fn a_prerelease_is_reachable_by_asking_for_it() {
        let release = exact(LS_REMOTE, "v0.3.0-rc.1").unwrap();
        assert_eq!(release.tag, "v0.3.0-rc.1");
        assert_eq!(release.version, None);
    }
}
