//! GitHub Releases is the single source of truth for IRIXEN Code versions.
//!
//! Two halves, deliberately separate. `check` only *reads* metadata and is
//! cheap enough to run on every visit to the About page. `download` fetches the
//! installer for this platform and hashes it before trusting it; `apply` starts
//! it and gets out of the way. A Windows exe cannot replace itself, so the
//! installer always runs as its own process — the app quits first and never
//! pretends it can swap its own binary.

use futures_util::StreamExt;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter};
use tokio::io::AsyncWriteExt;

use crate::net;

/// Product label shown everywhere in the UI. Package metadata must stay valid
/// semver, so the human-facing version lives here next to the release check.
pub const DISPLAY_VERSION: &str = "Beta 0.4";

pub const REPO_URL: &str = "https://github.com/lanche-furry/IRIXEN-Code";
pub const RELEASES_URL: &str = "https://github.com/lanche-furry/IRIXEN-Code/releases";
const API_LATEST: &str =
    "https://api.github.com/repos/lanche-furry/IRIXEN-Code/releases/latest";
const API_LIST: &str = "https://api.github.com/repos/lanche-furry/IRIXEN-Code/releases";
const CACHE_TTL: Duration = Duration::from_secs(600);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_NOTES: usize = 32 * 1024;

#[derive(Serialize, Clone, Default)]
#[serde(default)]
pub struct UpdateAsset {
    pub name: String,
    pub size: u64,
    pub download_url: String,
}

#[derive(Serialize, Clone, Default)]
#[serde(default)]
pub struct AppUpdate {
    /// Display label, e.g. "Beta 0.1".
    pub current: String,
    /// Packaged semver, e.g. "0.1.0-beta.1".
    pub current_semver: String,
    /// Raw tag name from GitHub, e.g. "v0.2.0".
    pub latest: String,
    pub latest_semver: String,
    pub name: String,
    pub published_at: String,
    pub notes: String,
    pub url: String,
    pub assets: Vec<UpdateAsset>,
    pub has_update: bool,
    pub prerelease: bool,
    pub checked_at: u64,
    pub releases_url: String,
    /// `/releases/latest` returns 404 for a repo with no published release.
    /// That is a known state, not a network failure.
    pub no_release: bool,
}

struct Cache {
    at: Instant,
    data: AppUpdate,
}
static CACHE: OnceLock<Mutex<Option<Cache>>> = OnceLock::new();

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn empty(current: String) -> AppUpdate {
    AppUpdate {
        current_semver: current,
        current: DISPLAY_VERSION.to_string(),
        releases_url: RELEASES_URL.to_string(),
        checked_at: now_secs(),
        ..Default::default()
    }
}

/// Query GitHub for the newest release. `force` bypasses the ten-minute cache;
/// a failed request is never cached, so a transient error cannot stick.
pub async fn check(proxy: &str, force: bool) -> Result<AppUpdate, String> {
    let cache = CACHE.get_or_init(|| Mutex::new(None));
    if !force {
        if let Ok(guard) = cache.lock() {
            if let Some(c) = guard.as_ref() {
                if c.at.elapsed() < CACHE_TTL {
                    return Ok(c.data.clone());
                }
            }
        }
    }

    let current = env!("CARGO_PKG_VERSION").to_string();
    let client = net::build_client(proxy);
    let resp = client
        .get(API_LATEST)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(|e| net::explain_transport(&e, API_LATEST, &net::proxy_label(proxy)))?;

    let status = resp.status();
    let body = resp
        .text()
        .await
        .map_err(|e| format!("读取 GitHub 响应失败：{e}"))?;

    let release: serde_json::Value = if status.is_success() {
        serde_json::from_str(&body).map_err(|e| format!("解析 GitHub 响应失败：{e}"))?
    } else if status.as_u16() == 404 {
        // `latest` omits repositories that only have drafts or prereleases.
        // A beta-only repo is exactly our case, so fall back to the list.
        match newest_from_list(&client, proxy).await? {
            Some(v) => v,
            None => {
                let mut out = empty(current);
                out.no_release = true;
                return Ok(out);
            }
        }
    } else {
        return Err(net::explain_status(status.as_u16(), &body, API_LATEST));
    };

    let out = from_release(&release, &current);
    if let Ok(mut guard) = cache.lock() {
        *guard = Some(Cache {
            at: Instant::now(),
            data: out.clone(),
        });
    }
    Ok(out)
}

async fn newest_from_list(
    client: &reqwest::Client,
    proxy: &str,
) -> Result<Option<serde_json::Value>, String> {
    let resp = client
        .get(API_LIST)
        .query(&[("per_page", "15")])
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(|e| net::explain_transport(&e, API_LIST, &net::proxy_label(proxy)))?;
    let status = resp.status();
    let body = resp
        .text()
        .await
        .map_err(|e| format!("读取 GitHub 响应失败：{e}"))?;
    if status.as_u16() == 404 {
        return Ok(None);
    }
    if !status.is_success() {
        return Err(net::explain_status(status.as_u16(), &body, API_LIST));
    }
    let rows: Vec<serde_json::Value> =
        serde_json::from_str(&body).map_err(|e| format!("解析 GitHub 响应失败：{e}"))?;
    // Daily CI builds live under the rolling `nightly` tag. Those are build
    // artifacts, not versions: `nightly` parses as a pre-release name that
    // sorts above every real tag, so offering it here would tell every user
    // they are out of date. Everything else that carries a version number is
    // fair game.
    Ok(rows.into_iter().find(|r| {
        if r.get("draft").and_then(|v| v.as_bool()) == Some(true) {
            return false;
        }
        is_version_tag(&str_field(r, "tag_name"))
    }))
}

/// Release tags follow the product label — `Beta0.1`, `Beta0.2` — or plain
/// semver (`v0.2.0`). Both are versions; `nightly` is the only tag that is
/// not. Testing "the first component is all digits" looked equivalent but
/// rejected `Beta0.1` itself, which silently disabled the update check for
/// every release cut from this repo's own naming scheme.
fn is_version_tag(tag: &str) -> bool {
    let head = tag.trim().trim_start_matches(['v', 'V']);
    if head.is_empty() || head.eq_ignore_ascii_case("nightly") {
        return false;
    }
    head.bytes().any(|b| b.is_ascii_digit())
}

/// Release tags repeat the product label: `Beta 0.2` is published as `Beta0.2`.
///
/// That label carries no prerelease suffix, so ordering the tag against the
/// packaged `0.2.0-beta.1` ranks it *above* the running build and the About page
/// would offer every user an "update" to the release they already installed.
/// Compare labels instead of versions for the exact-match case.
fn is_current_label(tag: &str, label: &str) -> bool {
    fn norm(s: &str) -> String {
        s.chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>()
            .to_lowercase()
    }
    let tag = norm(tag.trim().trim_start_matches(['v', 'V']));
    !tag.is_empty() && tag == norm(label)
}

fn str_field(v: &serde_json::Value, key: &str) -> String {
    v.get(key)
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string()
}

fn from_release(v: &serde_json::Value, current: &str) -> AppUpdate {
    let tag = str_field(v, "tag_name");
    let latest_semver = tag
        .trim_start_matches(['v', 'V'])
        .split('+')
        .next()
        .unwrap_or("")
        .to_string();
    let mut notes = str_field(v, "body");
    if notes.len() > MAX_NOTES {
        notes.truncate(MAX_NOTES);
        while !notes.is_char_boundary(notes.len()) {
            notes.pop();
        }
    }
    let assets = v
        .get("assets")
        .and_then(|a| a.as_array())
        .map(|rows| {
            rows.iter()
                .map(|a| UpdateAsset {
                    name: str_field(a, "name"),
                    size: a.get("size").and_then(|n| n.as_u64()).unwrap_or(0),
                    download_url: str_field(a, "browser_download_url"),
                })
                .filter(|a| !a.name.is_empty())
                .collect()
        })
        .unwrap_or_default();

    AppUpdate {
        current: DISPLAY_VERSION.to_string(),
        current_semver: current.to_string(),
        has_update: !is_current_label(&tag, DISPLAY_VERSION)
            && compare_semver(&latest_semver, current) == Ordering::Greater,
        latest: tag,
        latest_semver,
        name: str_field(v, "name"),
        published_at: str_field(v, "published_at"),
        notes,
        url: str_field(v, "html_url"),
        assets,
        prerelease: v.get("prerelease").and_then(|b| b.as_bool()) == Some(true),
        checked_at: now_secs(),
        releases_url: RELEASES_URL.to_string(),
        no_release: false,
    }
}

// ---------------------------------------------------------------------------
// download + apply
// ---------------------------------------------------------------------------

/// Download progress, throttled to a few events per second: a 90 MB AppImage
/// would otherwise push thousands of messages into the webview.
#[derive(Serialize, Clone, Default)]
pub struct DownloadProgress {
    pub received: u64,
    pub total: u64,
    pub asset: String,
}

#[derive(Serialize, Clone, Default)]
pub struct Downloaded {
    pub path: String,
    pub asset: String,
    /// True when a published checksum was matched; false only for an older
    /// release that shipped no `checksums.txt` at all.
    pub verified: bool,
}

/// The installer this build can actually run.
///
/// Windows publishes NSIS (`*-setup.exe`) only. Linux publishes an AppImage and
/// a deb: the deb needs root, so the AppImage is the one thing that can be
/// replaced by the same user who is running the app.
fn installer_asset(assets: &[UpdateAsset]) -> Option<&UpdateAsset> {
    #[cfg(windows)]
    {
        return assets
            .iter()
            .find(|a| a.name.to_ascii_lowercase().ends_with(".exe") && a.name.to_ascii_lowercase().contains("setup"));
    }
    #[cfg(target_os = "linux")]
    {
        return assets.iter().find(|a| a.name.ends_with(".AppImage"));
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = assets;
        None
    }
}

/// `checksums.txt` is `sha256  name` per line (the format `sha256sum` emits,
/// with an optional `*` before the name for binary mode).
fn parse_checksums(text: &str, file: &str) -> Option<String> {
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let hash = parts.next()?;
        let name = parts.next()?.trim_start_matches('*');
        if name == file && hash.len() == 64 && hash.chars().all(|c| c.is_ascii_hexdigit()) {
            return Some(hash.to_ascii_lowercase());
        }
    }
    None
}

fn hex(bytes: impl AsRef<[u8]>) -> String {
    use std::fmt::Write as _;
    bytes.as_ref().iter().fold(String::new(), |mut acc, b| {
        let _ = write!(acc, "{b:02x}");
        acc
    })
}

async fn fetch_text(client: &reqwest::Client, url: &str, proxy: &str) -> Result<String, String> {
    let resp = client
        .get(url)
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(|e| net::explain_transport(&e, url, &net::proxy_label(proxy)))?;
    let status = resp.status();
    let body = resp.text().await.map_err(|e| format!("读取响应失败：{e}"))?;
    if !status.is_success() {
        return Err(net::explain_status(status.as_u16(), &body, url));
    }
    Ok(body)
}

/// Fetch the installer for this platform into a temp file and hash it.
///
/// Both checks are load-bearing: the size comes from the release metadata, the
/// hash from `checksums.txt` when the release has one. A hash that disagrees
/// deletes the download rather than offering the user a choice — there is no
/// good reason to run a binary whose published hash it does not match.
pub async fn download(proxy: &str, app: &AppHandle) -> Result<Downloaded, String> {
    let latest = check(proxy, false).await?;
    if !latest.has_update {
        return Err("已经是最新版本".into());
    }
    let asset = installer_asset(&latest.assets)
        .ok_or("这个版本没有适用于当前平台的安装包，请到发布页手动下载")?
        .clone();
    let client = net::build_client(proxy);

    let expected = match latest
        .assets
        .iter()
        .find(|a| a.name.eq_ignore_ascii_case("checksums.txt"))
    {
        Some(list) => {
            let text = fetch_text(&client, &list.download_url, proxy).await?;
            parse_checksums(&text, &asset.name)
        }
        None => None,
    };

    let dir = std::env::temp_dir().join("irixen-update");
    std::fs::create_dir_all(&dir).map_err(|e| format!("无法创建下载目录：{e}"))?;
    let dest = dir.join(&asset.name);

    let resp = client
        .get(&asset.download_url)
        .timeout(Duration::from_secs(60))
        .send()
        .await
        .map_err(|e| net::explain_transport(&e, &asset.download_url, &net::proxy_label(proxy)))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(net::explain_status(status.as_u16(), "", &asset.download_url));
    }
    let total = resp.content_length().unwrap_or(asset.size);

    let mut file = tokio::fs::File::create(&dest)
        .await
        .map_err(|e| format!("无法写入 {}：{e}", dest.display()))?;
    let mut hasher = Sha256::new();
    let mut received: u64 = 0;
    let mut last = Instant::now();
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let bytes = chunk.map_err(|e| format!("下载中断：{e}"))?;
        hasher.update(&bytes);
        file.write_all(&bytes)
            .await
            .map_err(|e| format!("写入安装包失败：{e}"))?;
        received += bytes.len() as u64;
        if last.elapsed() >= Duration::from_millis(200) {
            last = Instant::now();
            let _ = app.emit(
                "update:progress",
                DownloadProgress { received, total, asset: asset.name.clone() },
            );
        }
    }
    file.flush().await.map_err(|e| format!("写入安装包失败：{e}"))?;
    drop(file);

    let actual = hex(hasher.finalize());
    let mut verified = false;
    if let Some(want) = expected {
        if want != actual {
            let _ = std::fs::remove_file(&dest);
            return Err(format!(
                "安装包校验失败：公布的 SHA256 是 {want}，下载到的是 {actual}。文件已删除。"
            ));
        }
        verified = true;
    }
    if asset.size > 0 && received != asset.size {
        let _ = std::fs::remove_file(&dest);
        return Err(format!(
            "下载不完整：应为 {} 字节，实际 {received} 字节。文件已删除。",
            asset.size
        ));
    }

    Ok(Downloaded {
        path: dest.to_string_lossy().to_string(),
        asset: asset.name.clone(),
        verified,
    })
}

/// Start the installer as its own process. The caller quits right after: the
/// file it is running from is about to be replaced.
pub fn apply(path: &str) -> Result<(), String> {
    #[cfg(windows)]
    {
        // No `/S`: a silent install leaves the user staring at a window that
        // vanished, with no idea whether anything happened.
        std::process::Command::new(path)
            .spawn()
            .map_err(|e| format!("无法启动安装程序：{e}"))?;
    }
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::PermissionsExt;
        let p = std::path::Path::new(path);
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("无法设置为可执行：{e}"))?;
        std::process::Command::new(p)
            .spawn()
            .map_err(|e| format!("无法启动安装包：{e}"))?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// semver
// ---------------------------------------------------------------------------

#[derive(Debug, PartialEq, Eq)]
enum Ident {
    Num(u64),
    Text(String),
}

fn parse_semver(raw: &str) -> (Vec<u64>, Vec<Ident>) {
    let s = raw.trim().trim_start_matches(['v', 'V']);
    let s = s.split('+').next().unwrap_or(s);
    let (core, pre) = s.split_once('-').unwrap_or((s, ""));
    let nums: Vec<u64> = core
        .split('.')
        .map(|p| p.parse::<u64>().unwrap_or(0))
        .collect();
    let ids: Vec<Ident> = if pre.is_empty() {
        Vec::new()
    } else {
        pre.split('.')
            .map(|p| match p.parse::<u64>() {
                Ok(n) => Ident::Num(n),
                Err(_) => Ident::Text(p.to_ascii_lowercase()),
            })
            .collect()
    };
    (nums, ids)
}

/// A prerelease sorts *below* its release (`beta.1 < 0.1.0`), numeric
/// identifiers sort numerically (`beta.2 > beta.10` is false), and a numeric
/// identifier always sorts below an alphabetic one.
pub fn compare_semver(a: &str, b: &str) -> Ordering {
    let (na, pa) = parse_semver(a);
    let (nb, pb) = parse_semver(b);
    for i in 0..na.len().max(nb.len()).max(3) {
        let cmp = na
            .get(i)
            .copied()
            .unwrap_or(0)
            .cmp(&nb.get(i).copied().unwrap_or(0));
        if cmp != Ordering::Equal {
            return cmp;
        }
    }
    match (pa.is_empty(), pb.is_empty()) {
        (true, true) => Ordering::Equal,
        (true, false) => Ordering::Greater,
        (false, true) => Ordering::Less,
        (false, false) => {
            for i in 0..pa.len().max(pb.len()) {
                match (pa.get(i), pb.get(i)) {
                    (None, None) => return Ordering::Equal,
                    (None, Some(_)) => return Ordering::Less,
                    (Some(_), None) => return Ordering::Greater,
                    (Some(x), Some(y)) => {
                        let cmp = match (x, y) {
                            (Ident::Num(m), Ident::Num(n)) => m.cmp(n),
                            (Ident::Text(m), Ident::Text(n)) => m.cmp(n),
                            (Ident::Num(_), Ident::Text(_)) => Ordering::Less,
                            (Ident::Text(_), Ident::Num(_)) => Ordering::Greater,
                        };
                        if cmp != Ordering::Equal {
                            return cmp;
                        }
                    }
                }
            }
            Ordering::Equal
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semver_order() {
        assert_eq!(compare_semver("0.2.0", "0.1.0-beta.1"), Ordering::Greater);
        assert_eq!(compare_semver("v0.1.0-beta.2", "0.1.0-beta.1"), Ordering::Greater);
        assert_eq!(compare_semver("0.1.0-beta.1", "0.1.0"), Ordering::Less);
        assert_eq!(compare_semver("0.1.0", "0.1.0"), Ordering::Equal);
        assert_eq!(compare_semver("0.1.0-beta.10", "0.1.0-beta.9"), Ordering::Greater);
        assert_eq!(compare_semver("1.0.0+build.3", "1.0.0"), Ordering::Equal);
    }

    /// The repo tags releases with the product label (`Beta0.1`), not semver.
    /// The update check must still see those, while the rolling CI tag must
    /// never be mistaken for a version.
    #[test]
    fn release_tags_are_versions_but_nightly_is_not() {
        assert!(is_version_tag("Beta0.1"));
        assert!(is_version_tag("Beta0.2"));
        assert!(is_version_tag("v0.2.0"));
        assert!(is_version_tag("0.2.0-beta.1"));
        assert!(!is_version_tag("nightly"));
        assert!(!is_version_tag("Nightly"));
        assert!(!is_version_tag(""));
    }

    #[test]
    fn product_label_tags_order_by_their_numbers() {
        assert_eq!(compare_semver("Beta0.2", "Beta0.1"), Ordering::Greater);
        assert_eq!(compare_semver("Beta0.2", "0.1.0-beta.1"), Ordering::Greater);
    }

    /// The release you are running is never an update to itself, however the
    /// tag happens to sort against the packaged semver.
    /// `checksums.txt` lists every asset; picking the wrong line would verify
    /// the installer against some other file's hash and refuse to install.
    #[test]
    fn a_checksum_is_matched_by_file_name() {
        let a = "a".repeat(64);
        let b = "b".repeat(64);
        let text = format!("{a}  IRIXEN.Code_x64-setup.exe\n{b}  *other.AppImage\n");
        assert_eq!(parse_checksums(&text, "IRIXEN.Code_x64-setup.exe"), Some(a));
        assert_eq!(parse_checksums(&text, "other.AppImage"), Some(b));
        assert_eq!(parse_checksums(&text, "absent.exe"), None);
        // A malformed line must not pass as a hash.
        assert_eq!(parse_checksums("deadbeef  x.exe", "x.exe"), None);
    }

    #[test]
    fn the_running_release_is_not_its_own_update() {
        assert!(is_current_label("Beta0.2", "Beta 0.2"));
        assert!(is_current_label("beta0.2", "Beta 0.2"));
        assert!(is_current_label("vBeta0.2", "Beta 0.2"));
        assert!(!is_current_label("Beta0.3", "Beta 0.2"));
        assert!(!is_current_label("nightly", "Beta 0.2"));
        assert_eq!(compare_semver("Beta0.2", "0.2.0-beta.1"), Ordering::Greater);
    }
}
