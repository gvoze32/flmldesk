//! Update checker: compares this build with the latest GitHub release.

use std::time::Duration;

use serde::Deserialize;

use crate::Error;

pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");

/// Release asset name fragment for this platform (matches the release workflow).
pub const PLATFORM: &str = if cfg!(target_os = "windows") {
    "windows"
} else if cfg!(target_os = "macos") {
    "macos"
} else {
    "linux"
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub tag: String,
    pub page_url: String,
    /// Download for this platform, if the release has one.
    pub asset_url: Option<String>,
}

#[derive(Deserialize)]
struct GitHubRelease {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<GitHubAsset>,
}

#[derive(Deserialize)]
struct GitHubAsset {
    name: String,
    browser_download_url: String,
}

/// `v1.2.3`, `1.2.3-beta` -> (1, 2, 3).
pub fn parse_version(text: &str) -> Option<(u64, u64, u64)> {
    let core = text.trim().trim_start_matches(['v', 'V']).split(['-', '+']).next()?;
    let mut parts = core.split('.').map(|p| p.parse::<u64>().ok());
    let version = (parts.next()??, parts.next().unwrap_or(Some(0))?, parts.next().unwrap_or(Some(0))?);
    parts.next().is_none().then_some(version)
}

/// Parse the GitHub "latest release" JSON; `Some` only when it is newer than `current`.
pub fn newer_release(json: &str, current: &str, platform: &str) -> Result<Option<Release>, Error> {
    let release: GitHubRelease =
        serde_json::from_str(json).map_err(|e| Error::new(format!("Respons GitHub tidak dikenali: {e}")))?;
    if release.draft || release.prerelease {
        return Ok(None);
    }
    let (Some(latest), Some(mine)) = (parse_version(&release.tag_name), parse_version(current)) else {
        return Ok(None);
    };
    if latest <= mine {
        return Ok(None);
    }
    let asset_url = release
        .assets
        .iter()
        .find(|a| a.name.to_lowercase().contains(platform))
        .map(|a| a.browser_download_url.clone());
    Ok(Some(Release { tag: release.tag_name, page_url: release.html_url, asset_url }))
}

/// `owner/repo` from the crate's `repository` URL.
pub fn repository_slug() -> Option<String> {
    let rest = REPOSITORY.trim_end_matches('/').trim_end_matches(".git");
    let rest = rest.strip_prefix("https://github.com/")?;
    (rest.split('/').count() == 2).then(|| rest.to_string())
}

/// Ask GitHub for the latest release (blocking; run off the UI thread).
pub fn check() -> Result<Option<Release>, Error> {
    let slug = repository_slug().ok_or_else(|| Error::new("Repository GitHub belum diatur di Cargo.toml."))?;
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(10))).build().into();
    let body = agent
        .get(format!("https://api.github.com/repos/{slug}/releases/latest"))
        .header("User-Agent", concat!("fl-ml-desk/", env!("CARGO_PKG_VERSION")))
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| Error::new(format!("Cek update gagal: {e}")))?
        .body_mut()
        .read_to_string()
        .map_err(|e| Error::new(format!("Cek update gagal: {e}")))?;
    newer_release(&body, CURRENT_VERSION, PLATFORM)
}

#[cfg(test)]
mod tests {
    use super::*;

    const JSON: &str = r#"{"tag_name":"v0.3.0","html_url":"https://github.com/o/r/releases/tag/v0.3.0",
        "draft":false,"prerelease":false,"assets":[
        {"name":"FLMLDesk-linux-x64.tar.gz","browser_download_url":"https://x/linux"},
        {"name":"FLMLDesk-windows-x64.zip","browser_download_url":"https://x/windows"}]}"#;

    #[test]
    fn newer_tag_offers_platform_asset() {
        let r = newer_release(JSON, "0.2.9", "windows").unwrap().unwrap();
        assert_eq!(r.tag, "v0.3.0");
        assert_eq!(r.asset_url.as_deref(), Some("https://x/windows"));
        assert_eq!(newer_release(JSON, "0.2.9", "macos").unwrap().unwrap().asset_url, None);
    }

    #[test]
    fn same_or_older_tag_is_not_an_update() {
        assert_eq!(newer_release(JSON, "0.3.0", "windows").unwrap(), None);
        assert_eq!(newer_release(JSON, "0.10.0", "windows").unwrap(), None);
    }

    #[test]
    fn version_parsing_is_numeric() {
        assert_eq!(parse_version("v0.10.2"), Some((0, 10, 2)));
        assert_eq!(parse_version("1.2.3-beta.1"), Some((1, 2, 3)));
        assert!(parse_version("0.10.0") > parse_version("0.9.9"));
        assert_eq!(parse_version("latest"), None);
    }
}
