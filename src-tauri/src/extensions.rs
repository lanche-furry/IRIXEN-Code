//! Third-party extensions: VS Code `.vsix`, Open VSX packages, and DSH plugins.
//!
//! The goal is *compatibility where it is honest*. A VS Code extension is a Node
//! program against an API we do not implement, so this module takes the part
//! that is declarative and can genuinely be honoured here — themes, snippets,
//! languages, command metadata — and records everything else as metadata instead
//! of pretending to run it.
//!
//! Two package shapes are supported:
//!   * `.vsix` — a ZIP with `extension/package.json` at the root of the payload
//!   * git repos — how DSH plugins ship (`dsh plugin add github:owner/repo`).
//!     Those are usually skill bundles, so every `SKILL.md` inside becomes a
//!     real skill the agent can match against.
//!
//! Everything is installed under `<config>/extensions/<dir-id>/` with a
//! `.irixen-ext.json` sidecar, so listing is a directory scan rather than a
//! re-parse of every manifest.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::net;

const META_FILE: &str = ".irixen-ext.json";
const OPEN_VSX: &str = "https://open-vsx.org/api";
const MAX_TOTAL_BYTES: u64 = 512 * 1024 * 1024;
const MAX_FILES: usize = 20_000;

// ---------------------------------------------------------------------------
// model
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(default)]
pub struct ExtContributions {
    pub commands: Vec<ExtCommand>,
    pub themes: Vec<ExtTheme>,
    pub snippets: Vec<ExtSnippet>,
    pub languages: Vec<ExtLanguage>,
    /// Right-click / title-bar / command-palette entries declared by extension.
    pub menus: Vec<ExtMenu>,
    /// `contributes.submenus` declarations, referenced by `menus` rows whose
    /// `submenu` field is set instead of `command`.
    pub submenus: Vec<ExtSubmenu>,
    /// Activity-bar / panel containers declared by `viewsContainers`.
    #[serde(default, rename = "viewContainers")]
    pub view_containers: Vec<ExtViewContainer>,
    /// Individual views inside those containers, keyed by container id.
    #[serde(default)]
    pub views: Vec<ExtView>,
    /// Relative paths of `SKILL.md` files imported as agent skills.
    pub skills: Vec<String>,
}

/// One `contributes.viewsContainers.*` entry. `icon` is resolved to an absolute
/// path at parse time: the renderer can only load it through the asset protocol.
#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(default)]
pub struct ExtViewContainer {
    pub id: String,
    pub title: String,
    pub icon: String,
}

/// One `contributes.views.<container>` entry.
#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(default)]
pub struct ExtView {
    /// Id of the owning `viewsContainers` entry.
    pub container: String,
    pub id: String,
    pub name: String,
    /// Raw `when` clause; the renderer evaluates the subset it understands.
    pub when: String,
    /// `collapsed` / `visible` / `hidden` from the manifest.
    pub visibility: String,
}

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(default)]
pub struct ExtSubmenu {
    pub id: String,
    pub label: String,
}

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(default)]
pub struct ExtCommand {
    pub command: String,
    pub title: String,
    pub category: String,
    /// Absolute path of the command's icon, empty when the extension has none.
    pub icon: String,
}

/// One `contributes.menus` row. `when` is kept raw: the renderer decides whether
/// the row applies to the file or editor being right-clicked.
#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(default)]
pub struct ExtMenu {
    /// `editor/context`, `explorer/context`, `view/item/context`, `commandPalette`, …
    pub menu: String,
    /// Empty when this row opens a submenu instead of running a command.
    pub command: String,
    /// `contributes.menus` row that points at a `contributes.submenus` id.
    pub submenu: String,
    /// `navigation`, `1_modification`, … — ordering hint, not a label.
    pub group: String,
    pub when: String,
}

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(default)]
pub struct ExtTheme {
    pub id: String,
    pub label: String,
    pub path: String,
}

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(default)]
pub struct ExtSnippet {
    pub language: String,
    pub path: String,
}

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(default)]
pub struct ExtLanguage {
    pub id: String,
    pub aliases: Vec<String>,
    pub extensions: Vec<String>,
}

/// Machine-readable capability note. The UI renders it as a localized
/// sentence, so nothing here is prose and nothing here is persisted — it is a
/// pure function of `contributes` and `kind`, and older sidecars therefore pick
/// it up for free.
#[derive(Serialize, Clone, Default)]
pub struct ExtNotice {
    /// `capabilities` | `meta_only` | `skills` | `no_skills`
    pub kind: String,
    /// capability ids: `themes` | `snippets` | `languages`
    pub parts: Vec<String>,
    /// skill packs imported (only for `kind == "skills"`)
    pub count: i64,
}

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(default)]
pub struct ExtensionInfo {
    pub id: String,
    pub dir: String,
    pub name: String,
    pub publisher: String,
    pub version: String,
    pub display_name: String,
    pub description: String,
    /// Absolute path of the extension's icon (`package.json#icon`, falling back
    /// to `icon.png` in the root). Empty when the package ships no icon.
    #[serde(default)]
    pub icon: String,
    /// `vscode` | `dsh`
    pub kind: String,
    /// `vsix` | `online` | `git`
    pub source: String,
    pub enabled: bool,
    pub installed_at: u64,
    pub contributes: ExtContributions,
    /// `main` from package.json, relative to the extension dir. Empty for purely
    /// declarative packages (themes/snippets need no code).
    #[serde(default)]
    pub entry: String,
    /// `activationEvents`, e.g. `["onStartupFinished", "onCommand:x"]`.
    #[serde(default)]
    pub activation: Vec<String>,
    /// `main` is declared but not on disk. Means the package was unpacked
    /// incompletely (an interrupted install, or a byte budget in an older
    /// extractor). Computed on read, so repairing the files clears it with no
    /// extra bookkeeping.
    #[serde(default)]
    pub broken: bool,
    /// Honest capability note shown in the UI, e.g. why a Node extension will
    /// not run here. Derived on read (`meta_of`), so a stale sidecar can never
    /// carry an out-of-date note.
    #[serde(skip_deserializing)]
    pub notice: ExtNotice,
}

#[derive(Serialize, Clone, Default)]
pub struct ExtSearchRow {
    pub id: String,
    pub namespace: String,
    pub name: String,
    pub display_name: String,
    pub description: String,
    pub version: String,
    pub downloads: i64,
    pub rating: f64,
    pub timestamp: String,
    /// Icon URL from Open VSX (absolute, may be empty).
    pub icon: String,
    // No `node` flag here on purpose: the /-/search response has no `main`
    // or `engines` field, and `engines.vscode` (available on /api/{id}) is set
    // by pure theme extensions too, so neither can tell us whether a VSIX
    // ships JS. The detail pane answers that by reading its manifest.
    pub installed_version: String,
}

/// An installed extension whose Open VSX version is newer than the local copy.
#[derive(Serialize, Clone)]
pub struct ExtUpdate {
    pub id: String,
    pub current: String,
    pub latest: String,
}

/// Everything the store's detail pane needs, in one round trip.
#[derive(Serialize, Clone, Default)]
pub struct ExtDetail {
    pub id: String,
    pub namespace: String,
    pub name: String,
    pub display_name: String,
    pub description: String,
    pub version: String,
    pub versions: Vec<String>,
    pub downloads: i64,
    pub rating: f64,
    pub reviews: i64,
    pub license: String,
    pub repository: String,
    pub homepage: String,
    pub categories: Vec<String>,
    pub readme: String,
    pub node: bool,
    pub installed_version: String,
}

#[derive(Serialize, Clone)]
pub struct SnippetRow {
    pub ext: String,
    pub language: String,
    pub prefix: String,
    pub body: String,
    pub description: String,
}

// ---------------------------------------------------------------------------
// registry (directory scan)
// ---------------------------------------------------------------------------

pub fn dir() -> PathBuf {
    crate::config::extensions_dir()
}

fn meta_of(dir: &Path) -> Option<ExtensionInfo> {
    // The manifest decides whether this directory is an extension at all. The
    // sidecar only carries what a manifest cannot express — where it came from,
    // whether the user disabled it, when it was installed — so a sidecar that
    // is missing (install interrupted between unpack and metadata, a directory
    // copied in by hand) must not make the extension invisible. That failure
    // mode is indistinguishable from "the extension does nothing".
    let bytes = std::fs::read(dir.join("package.json")).ok()?;
    let pkg: PackageJson = serde_json::from_slice(&bytes).ok()?;
    let locale = ui_locale();
    let nls = nls_map(dir, &locale);

    let mut info = std::fs::read_to_string(dir.join(META_FILE))
        .ok()
        .and_then(|raw| serde_json::from_str::<ExtensionInfo>(&raw).ok())
        .unwrap_or_else(|| ExtensionInfo {
            id: format!("{}.{}", pkg.publisher, pkg.name),
            source: "vsix".into(),
            enabled: true,
            installed_at: file_mtime(&dir.join("package.json")),
            ..ExtensionInfo::default()
        });

    // Identity comes from the manifest: a sidecar left over from an older
    // version would otherwise keep re-labelling the extension.
    info.dir = dir.to_string_lossy().to_string();
    if !pkg.name.is_empty() {
        info.name = pkg.name.clone();
    }
    if !pkg.publisher.is_empty() {
        info.publisher = pkg.publisher.clone();
        info.id = format!("{}.{}", pkg.publisher, pkg.name);
    }
    if !pkg.version.is_empty() {
        info.version = pkg.version.clone();
    }
    // Recomputing contributions here is what makes newly supported contribution
    // points (`menus`, command icons) show up for extensions installed before
    // that support existed, without asking the user to reinstall anything.
    let skills = info.contributes.skills.clone();
    info.icon = icon_path(dir, &pkg);
    info.contributes = contributions(&pkg, dir, &locale);
    info.contributes.skills = skills;
    // `displayName` and `description` are localised the same way as command
    // titles — a `%key%` left raw shows up as a list entry whose name looks
    // broken.
    info.display_name = {
        let name = localize_str(&pkg.display_name, &nls);
        if name.is_empty() { info.name.clone() } else { name }
    };
    info.description = localize_str(&pkg.description, &nls);
    info.entry = pkg.main.trim_start_matches("./").replace('\\', "/");
    if info.activation.is_empty() {
        info.activation = pkg.activation_events.clone();
    }
    info.notice = notice_for(&info);
    // Reported here rather than at activation time: "Cannot find module
    // …/dist/index.js" looks like a broken host, when in fact the download was
    // incomplete. The list can say so and offer a reinstall.
    info.broken = !info.entry.is_empty() && !dir.join(&info.entry).is_file();
    Some(info)
}

fn write_meta(dir: &Path, info: &ExtensionInfo) -> Result<(), String> {
    let raw = serde_json::to_string_pretty(info).map_err(|e| e.to_string())?;
    std::fs::write(dir.join(META_FILE), raw).map_err(|e| format!("写入扩展元数据失败：{e}"))
}

/// Install time for packages that have no sidecar: the manifest's own mtime is
/// the closest honest answer, and it only feeds "recently installed" ordering.
fn file_mtime(path: &Path) -> u64 {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub fn list() -> Vec<ExtensionInfo> {
    let root = dir();
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(&root) else {
        return out;
    };
    for entry in entries.flatten() {
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        if let Some(info) = meta_of(&entry.path()) {
            out.push(info);
        }
    }
    out.sort_by(|a, b| a.display_name.to_lowercase().cmp(&b.display_name.to_lowercase()));
    out
}

pub fn find(id: &str) -> Option<ExtensionInfo> {
    list().into_iter().find(|e| e.id == id)
}

fn find_dir(id: &str) -> Result<(PathBuf, ExtensionInfo), String> {
    let root = dir();
    if let Ok(entries) = std::fs::read_dir(&root) {
        for entry in entries.flatten() {
            let p = entry.path();
            if !p.is_dir() {
                continue;
            }
            if let Some(info) = meta_of(&p) {
                if info.id == id {
                    return Ok((p, info));
                }
            }
        }
    }
    Err(format!("未安装扩展 {id}"))
}

pub fn set_enabled(id: &str, enabled: bool) -> Result<ExtensionInfo, String> {
    let (path, mut info) = find_dir(id)?;
    info.enabled = enabled;
    write_meta(&path, &info)?;
    Ok(info)
}

pub fn uninstall(id: &str) -> Result<(), String> {
    let (path, info) = find_dir(id)?;
    // Skills imported from this plugin live in our own skills dir; drop them
    // too, otherwise the agent keeps seeing a plugin that is gone.
    for rel in &info.contributes.skills {
        let slug = rel
            .trim_end_matches("SKILL.md")
            .trim_end_matches('/')
            .replace(['/', '\\'], "-");
        let target = crate::config::skills_dir().join(format!("{}-{}", info.id, slug));
        let _ = std::fs::remove_dir_all(target);
    }
    std::fs::remove_dir_all(&path).map_err(|e| format!("删除失败：{e}"))
}

// ---------------------------------------------------------------------------
// manifest parsing
// ---------------------------------------------------------------------------

#[derive(Deserialize, Default)]
#[serde(default)]
struct PackageJson {
    name: String,
    publisher: String,
    version: String,
    #[serde(rename = "displayName")]
    display_name: String,
    description: String,
    engines: serde_json::Value,
    contributes: serde_json::Value,
    /// String path or `{ light, dark }`.
    #[serde(default)]
    icon: serde_json::Value,
    #[serde(default)]
    main: String,
    #[serde(default, rename = "activationEvents")]
    activation_events: Vec<String>,
}

fn str_field(v: &serde_json::Value, key: &str) -> String {
    v.get(key).and_then(|x| x.as_str()).unwrap_or("").to_string()
}

/* --------------------------------------------------------- manifest l10n */

/// Resolve `%key%` placeholders the way VS Code does before it hands a
/// `package.json` to anyone.
///
/// Microsoft's own extensions put *every* command title behind a key
/// (`"%commands.startServer%"`) and ship the text in `package.nls.json` plus
/// `package.nls.<locale>.json`. Without this step those titles reach the UI as
/// literal `%commands.startServer%`, and rows whose key is missing look like
/// controls that lost their name. `{ "value": …, "original": … }` wrappers
/// (the l10n form of a single string) collapse to `value` here too.
fn nls_map(dir: &Path, locale: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut read = |name: &str| {
        let Ok(text) = std::fs::read_to_string(dir.join(name)) else {
            return;
        };
        let Ok(map) = serde_json::from_str::<BTreeMap<String, serde_json::Value>>(&text) else {
            return;
        };
        for (k, v) in map {
            if let Some(s) = v.as_str() {
                out.insert(k, s.to_string());
            }
        }
    };
    // Base file first (usually English), then the closest translation on top.
    read("package.nls.json");
    for cand in locale_candidates(locale) {
        let name = format!("package.nls.{cand}.json");
        if dir.join(&name).exists() {
            read(&name);
            break;
        }
    }
    out
}

/// `zh-CN` → `["zh-cn"]`, `en-US` → `["en-us", "en"]` (regional, then base).
fn locale_candidates(locale: &str) -> Vec<String> {
    let low = locale.trim().to_lowercase().replace('_', "-");
    if low.is_empty() {
        return vec!["en".into()];
    }
    let mut out = vec![low.clone()];
    if let Some((base, _)) = low.split_once('-') {
        out.push(base.to_string());
    }
    out
}

fn localize(v: &serde_json::Value, nls: &BTreeMap<String, String>) -> serde_json::Value {
    use serde_json::Value;
    match v {
        Value::String(s) => {
            let replaced = match s.strip_prefix('%').and_then(|r| r.strip_suffix('%')) {
                Some(key) if !key.is_empty() => nls.get(key).cloned().unwrap_or_else(|| s.clone()),
                _ => s.clone(),
            };
            Value::String(replaced)
        }
        Value::Array(items) => Value::Array(items.iter().map(|x| localize(x, nls)).collect()),
        Value::Object(map) => {
            // A localised string, not a real object: `{ value, original }`.
            if let (Some(Value::String(val)), Some(_)) = (map.get("value"), map.get("original")) {
                return Value::String(val.clone());
            }
            Value::Object(
                map.iter()
                    .map(|(k, x)| (k.clone(), localize(x, nls)))
                    .collect(),
            )
        }
        other => other.clone(),
    }
}

/// Localise a whole `package.json` before it is handed to an extension host —
/// `extension.packageJSON` and `extensions.all[].packageJSON` must expose the
/// same text the marketplace UI shows, not `%key%` placeholders.
pub(crate) fn localize_manifest(
    pkg: &serde_json::Value,
    dir: &Path,
    locale: &str,
) -> serde_json::Value {
    localize(pkg, &nls_map(dir, locale))
}

/// The UI language, or `en` when the config cannot be read.
fn ui_locale() -> String {
    crate::config::AppConfig::load().language
}

/// Localise one standalone string (`displayName`, `description`).
fn localize_str(s: &str, nls: &BTreeMap<String, String>) -> String {
    match s.strip_prefix('%').and_then(|r| r.strip_suffix('%')) {
        Some(key) if !key.is_empty() => nls.get(key).cloned().unwrap_or_else(|| s.to_string()),
        _ => s.to_string(),
    }
}

/// `icon` is either a relative path or `{ "light": …, "dark": … }`.
fn icon_rel(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Object(o) => o
            .get("light")
            .and_then(|x| x.as_str())
            .or_else(|| o.get("dark").and_then(|x| x.as_str()))
            .unwrap_or_default()
            .to_string(),
        _ => String::new(),
    }
}

/// Resolve a manifest-relative file to an absolute path, or "" when absent.
/// Plain (non-verbatim) form: the renderer turns it into an asset URL, and
/// `\\?\` paths do not survive that conversion.
fn file_rel(dir: &Path, rel: &str) -> String {
    if rel.is_empty() {
        return String::new();
    }
    let joined = dir.join(rel.trim_start_matches("./").replace('\\', "/"));
    if joined.is_file() {
        crate::util::plain_path(&joined.to_string_lossy())
    } else {
        String::new()
    }
}

/// The extension's own icon: `package.json#icon`, else `icon.png` in the root.
fn icon_path(dir: &Path, pkg: &PackageJson) -> String {
    let rel = icon_rel(&pkg.icon);
    let found = file_rel(dir, &rel);
    if !found.is_empty() {
        return found;
    }
    file_rel(dir, "icon.png")
}

fn str_list(v: Option<&serde_json::Value>) -> Vec<String> {
    match v {
        Some(serde_json::Value::String(s)) => vec![s.clone()],
        Some(serde_json::Value::Array(a)) => a
            .iter()
            .filter_map(|x| x.as_str().map(|s| s.to_string()))
            .collect(),
        _ => Vec::new(),
    }
}

fn contributions(pkg: &PackageJson, dir: &Path, locale: &str) -> ExtContributions {
    // Everything downstream reads the localised manifest, so command titles,
    // theme labels and language aliases all come out as real text.
    let localized = localize(&pkg.contributes, &nls_map(dir, locale));
    let c = &localized;
    let mut out = ExtContributions::default();

    if let Some(items) = c.get("commands").and_then(|v| v.as_array()) {
        for it in items {
            let command = str_field(it, "command");
            if command.is_empty() {
                continue;
            }
            out.commands.push(ExtCommand {
                command,
                title: str_field(it, "title"),
                category: str_field(it, "category"),
                icon: file_rel(dir, &icon_rel(it.get("icon").unwrap_or(&serde_json::Value::Null))),
            });
        }
    }

    if let Some(items) = c.get("submenus").and_then(|v| v.as_array()) {
        for it in items {
            let id = str_field(it, "id");
            if id.is_empty() {
                continue;
            }
            out.submenus.push(ExtSubmenu { id, label: str_field(it, "label") });
        }
    }

    // Keep every location, not just the three we render today: `commandPalette`
    // rows gate visibility of internal commands, and rows keyed by a submenu id
    // are the children of a `submenu` row. Dropping either here is what used to
    // leave an unnamed right-click item and a palette full of `_gradle.*` RPCs.
    if let Some(map) = c.get("menus").and_then(|v| v.as_object()) {
        for (menu, rows) in map {
            let Some(rows) = rows.as_array() else { continue };
            for it in rows {
                let command = str_field(it, "command");
                let submenu = str_field(it, "submenu");
                if command.is_empty() && submenu.is_empty() {
                    continue;
                }
                out.menus.push(ExtMenu {
                    menu: menu.clone(),
                    command,
                    submenu,
                    group: str_field(it, "group"),
                    when: str_field(it, "when"),
                });
            }
        }
    }

    // Views are declarative, so they must be visible in the activity bar even
    // before the extension is activated (and even when its host fails). Parsing
    // them here — rather than at activation time — is also what makes older
    // installations pick up the contribution without a reinstall.
    if let Some(map) = c.get("viewsContainers").and_then(|v| v.as_object()) {
        for rows in map.values() {
            let Some(rows) = rows.as_array() else { continue };
            for it in rows {
                let id = str_field(it, "id");
                if id.is_empty() {
                    continue;
                }
                out.view_containers.push(ExtViewContainer {
                    id,
                    title: str_field(it, "title"),
                    icon: file_rel(dir, &icon_rel(it.get("icon").unwrap_or(&serde_json::Value::Null))),
                });
            }
        }
    }
    if let Some(map) = c.get("views").and_then(|v| v.as_object()) {
        for (container, rows) in map {
            let Some(rows) = rows.as_array() else { continue };
            for it in rows {
                let id = str_field(it, "id");
                if id.is_empty() {
                    continue;
                }
                out.views.push(ExtView {
                    container: container.clone(),
                    id,
                    name: str_field(it, "name"),
                    when: str_field(it, "when"),
                    visibility: str_field(it, "visibility"),
                });
            }
        }
    }

    if let Some(items) = c.get("themes").and_then(|v| v.as_array()) {
        for it in items {
            let path = str_field(it, "path");
            if path.is_empty() {
                continue;
            }
            out.themes.push(ExtTheme {
                id: str_field(it, "id"),
                label: str_field(it, "label"),
                path,
            });
        }
    }

    if let Some(items) = c.get("snippets").and_then(|v| v.as_array()) {
        for it in items {
            let path = str_field(it, "path");
            if path.is_empty() {
                continue;
            }
            out.snippets.push(ExtSnippet {
                language: str_field(it, "language"),
                path,
            });
        }
    }

    if let Some(items) = c.get("languages").and_then(|v| v.as_array()) {
        for it in items {
            let id = str_field(it, "id");
            if id.is_empty() {
                continue;
            }
            out.languages.push(ExtLanguage {
                id,
                aliases: str_list(it.get("aliases")),
                extensions: str_list(it.get("extensions")),
            });
        }
    }

    out
}

// ---------------------------------------------------------------------------
// install — entry point
// ---------------------------------------------------------------------------

/// Install from any of: a `.vsix` path, a `.vsix` URL, `publisher.name[@ver]`
/// (Open VSX), `github:owner/repo`, or a git URL.
pub async fn install(source: &str, proxy: &str) -> Result<ExtensionInfo, String> {
    let src = source.trim();
    if src.is_empty() {
        return Err("请填写扩展来源".into());
    }

    let as_path = PathBuf::from(src);
    if as_path.is_file() {
        return install_vsix(&as_path, "vsix");
    }

    let lower = src.to_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        if lower.split('?').next().unwrap_or("").ends_with(".vsix") {
            let bytes = download(src, proxy).await?;
            return install_vsix_bytes(&bytes, "online");
        }
        if lower.contains("github.com/") || lower.ends_with(".git") {
            return install_git(src, proxy).await;
        }
        return Err("只支持 .vsix 直链或 GitHub 仓库地址".into());
    }

    if lower.starts_with("github:") || lower.starts_with("git@") {
        return install_git(src, proxy).await;
    }

    // `owner/repo` — git, unless it looks like `publisher.name`.
    if src.contains('/') && !src.contains('.') {
        return install_git(&format!("https://github.com/{src}"), proxy).await;
    }
    if src.contains('/') {
        return install_git(&format!("https://github.com/{src}"), proxy).await;
    }

    if src.contains('.') {
        return install_online(src, proxy).await;
    }

    Err("无法识别来源。可以是：.vsix 路径 / publisher.name / github:owner/repo".into())
}

async fn download(url: &str, proxy: &str) -> Result<Vec<u8>, String> {
    let client = net::build_client(proxy);
    let resp = client
        .get(url)
        .header("Accept", "application/octet-stream")
        .send()
        .await
        .map_err(|e| net::explain_transport(&e, url, &net::proxy_label(proxy)))?;
    if !resp.status().is_success() {
        return Err(format!("下载失败（HTTP {}）：{url}", resp.status().as_u16()));
    }
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("下载中断：{e}"))?;
    Ok(bytes.to_vec())
}

async fn install_online(spec: &str, proxy: &str) -> Result<ExtensionInfo, String> {
    let (id, version) = match spec.split_once('@') {
        Some((i, v)) => (i, Some(v)),
        None => (spec, None),
    };
    let (publisher, name) = id
        .split_once('.')
        .ok_or_else(|| "在线安装需要 publisher.name 形式".to_string())?;
    let url = match version {
        Some(v) => format!("{OPEN_VSX}/{publisher}/{name}/{v}"),
        None => format!("{OPEN_VSX}/{publisher}/{name}/latest"),
    };
    let client = net::build_client(proxy);
    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| net::explain_transport(&e, &url, &net::proxy_label(proxy)))?;
    if resp.status().as_u16() == 404 {
        return Err(format!("Open VSX 上没有 {id}"));
    }
    if !resp.status().is_success() {
        return Err(format!("查询失败（HTTP {}）", resp.status().as_u16()));
    }
    let meta: serde_json::Value = resp.json().await.map_err(|e| format!("解析响应失败：{e}"))?;
    let dl = meta
        .get("files")
        .and_then(|f| f.get("download"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| "该扩展没有可下载的包".to_string())?
        .to_string();
    let bytes = download(&dl, proxy).await?;
    install_vsix_bytes(&bytes, "online")
}

fn install_vsix(path: &Path, source: &str) -> Result<ExtensionInfo, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("读取失败：{e}"))?;
    install_vsix_bytes(&bytes, source)
}

fn install_vsix_bytes(bytes: &[u8], source: &str) -> Result<ExtensionInfo, String> {
    let entries = zip::list(bytes)?;
    let manifest_name = entries
        .iter()
        .find(|e| e.name == "extension/package.json")
        .or_else(|| entries.iter().find(|e| e.name == "package.json"))
        .map(|e| e.name.clone())
        .ok_or_else(|| "这个包里没有 package.json，不是有效的 VSIX".to_string())?;
    let raw = zip::read(bytes, entries.iter().find(|e| e.name == manifest_name).unwrap())?;
    let pkg: PackageJson =
        serde_json::from_slice(&raw).map_err(|e| format!("package.json 解析失败：{e}"))?;

    let publisher = if pkg.publisher.is_empty() {
        "unknown".to_string()
    } else {
        pkg.publisher.clone()
    };
    let name = if pkg.name.is_empty() {
        "extension".to_string()
    } else {
        pkg.name.clone()
    };
    let version = if pkg.version.is_empty() {
        "0.0.0".to_string()
    } else {
        pkg.version.clone()
    };
    let id = format!("{publisher}.{name}");
    let dir_id = sanitize(&format!("{id}-{version}"));

    let root = dir().join(&dir_id);
    if root.exists() {
        std::fs::remove_dir_all(&root).map_err(|e| format!("清理旧版本失败：{e}"))?;
    }
    std::fs::create_dir_all(&root).map_err(|e| format!("创建目录失败：{e}"))?;

    // Strip the leading `extension/` so paths in `contributes` resolve directly.
    let prefix = if manifest_name == "extension/package.json" {
        "extension/"
    } else {
        ""
    };
    let mut written = 0usize;
    let mut total = 0u64;
    for entry in &entries {
        if !entry.name.starts_with(prefix) || entry.name.ends_with('/') {
            continue;
        }
        let rel = &entry.name[prefix.len()..];
        let Some(rel_path) = safe_rel(rel) else {
            continue;
        };
        if written >= MAX_FILES || total > MAX_TOTAL_BYTES {
            break;
        }
        let data = match zip::read(bytes, entry) {
            Ok(d) => d,
            Err(_) => continue,
        };
        total += data.len() as u64;
        let target = root.join(&rel_path);
        if let Some(parent) = target.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if std::fs::write(&target, &data).is_ok() {
            written += 1;
        }
    }
    if written == 0 {
        return Err("包里没有可解出的文件".into());
    }
    // A manifest that names a `main` the archive did not deliver is a package
    // we cannot activate. Reporting it now beats a later "Cannot find module"
    // from a host that looks healthy.
    if !pkg.main.trim().is_empty() {
        let rel = pkg.main.trim_start_matches("./").replace('\\', "/");
        if !root.join(&rel).is_file() {
            let _ = std::fs::remove_dir_all(&root);
            return Err(format!(
                "安装包不完整：package.json 声明了 {rel}，但压缩包里没有这个文件"
            ));
        }
    }

    let nls = nls_map(&root, &ui_locale());
    let mut info = ExtensionInfo {
        id,
        dir: root.to_string_lossy().to_string(),
        name,
        publisher,
        version,
        display_name: {
            let name = localize_str(&pkg.display_name, &nls);
            if name.is_empty() { pkg.name.clone() } else { name }
        },
        description: localize_str(&pkg.description, &nls),
        icon: icon_path(&root, &pkg),
        kind: "vscode".into(),
        source: source.into(),
        enabled: true,
        installed_at: crate::util::now_ms(),
        contributes: contributions(&pkg, &root, &ui_locale()),
        entry: pkg.main.trim_start_matches("./").replace('\\', "/"),
        activation: pkg.activation_events.clone(),
        broken: false,
        notice: ExtNotice::default(),
    };
    info.notice = notice_for(&info);
    write_meta(&root, &info)?;
    Ok(info)
}

/// Say plainly what will and will not work, so nobody expects a Node extension
/// host to materialise. Returns tokens, not sentences — the dictionary owns the
/// wording, so this stays out of the i18n gap.
fn notice_for(info: &ExtensionInfo) -> ExtNotice {
    let skills = info.skill_count();
    if skills > 0 {
        return ExtNotice { kind: "skills".into(), count: skills as i64, ..Default::default() };
    }

    let mut parts = Vec::new();
    if !info.contributes.themes.is_empty() {
        parts.push("themes".to_string());
    }
    if !info.contributes.snippets.is_empty() {
        parts.push("snippets".to_string());
    }
    if !info.contributes.languages.is_empty() {
        parts.push("languages".to_string());
    }

    // JavaScript changes the story completely: the extension is not just a set
    // of declarative contributions any more, it runs in a host process.
    if !info.entry.is_empty() {
        return ExtNotice { kind: "hosted".into(), parts, count: 0 };
    }
    if info.kind != "vscode" {
        return ExtNotice { kind: "no_skills".into(), ..Default::default() };
    }
    if parts.is_empty() {
        return ExtNotice { kind: "meta_only".into(), ..Default::default() };
    }
    ExtNotice { kind: "capabilities".into(), parts, count: 0 }
}

/// Install a git repo (the shape DSH plugins ship in).
///
/// GitHub repos are fetched as a zip from codeload and unpacked with the same
/// ZIP reader the `.vsix` path uses. That is deliberate: `git clone` goes
/// through OpenSSL's own CA bundle, which a TLS-intercepting corporate proxy
/// breaks ("certificate signer not trusted"), while our HTTP client follows the
/// same trust path as the rest of the app. Non-GitHub URLs fall back to `git`.
async fn install_git(url: &str, proxy: &str) -> Result<ExtensionInfo, String> {
    let (owner, repo) = parse_repo(url)?;
    let slug = format!("{owner}.{repo}");

    let zip_url = format!("https://codeload.github.com/{owner}/{repo}/zip/HEAD");
    let bytes = match download(&zip_url, proxy).await {
        Ok(b) => b,
        Err(e) if url.starts_with("github:") || url.contains("github.com") => {
            return Err(format!("下载仓库失败：{e}"));
        }
        Err(_) => return clone_repo(url, proxy, &slug).await,
    };

    let entries = zip::list(&bytes)?;
    // GitHub zips wrap everything in `<repo>-HEAD/`; strip that one component.
    let prefix = entries
        .first()
        .map(|e| e.name.split('/').next().unwrap_or("").to_string())
        .filter(|p| !p.is_empty())
        .map(|p| format!("{p}/"))
        .unwrap_or_default();

    let root = dir().join(sanitize(&slug));
    if root.exists() {
        let _ = std::fs::remove_dir_all(&root);
    }
    std::fs::create_dir_all(&root).map_err(|e| format!("创建目录失败：{e}"))?;

    let mut written = 0usize;
    let mut total = 0u64;
    for entry in &entries {
        if !entry.name.starts_with(&prefix) || entry.name.ends_with('/') {
            continue;
        }
        let rel = &entry.name[prefix.len()..];
        let Some(rel_path) = safe_rel(rel) else { continue };
        if written >= MAX_FILES || total > MAX_TOTAL_BYTES {
            break;
        }
        let data = match zip::read(&bytes, entry) {
            Ok(d) => d,
            Err(_) => continue,
        };
        total += data.len() as u64;
        let target = root.join(&rel_path);
        if let Some(parent) = target.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if std::fs::write(&target, &data).is_ok() {
            written += 1;
        }
    }
    if written == 0 {
        let _ = std::fs::remove_dir_all(&root);
        return Err("下载到的仓库是空的".into());
    }
    finish_repo(root, slug)
}

/// `github:owner/repo`, `https://github.com/owner/repo(.git)`, `owner/repo`
/// or an SSH URL all end up as (owner, repo).
fn parse_repo(url: &str) -> Result<(String, String), String> {
    let raw = url
        .trim()
        .trim_start_matches("github:")
        .trim_end_matches('/')
        .trim_end_matches(".git");
    let cleaned = raw
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_start_matches("git@")
        .replace(':', "/");
    let parts: Vec<&str> = cleaned.split('/').filter(|p| !p.is_empty()).collect();
    if parts.len() < 2 {
        return Err("地址里找不到 owner/repo".into());
    }
    // For URLs take the last two segments; for `github:owner/repo` likewise.
    let owner = parts[parts.len() - 2];
    let repo = parts[parts.len() - 1];
    if owner.contains('.') && parts.len() == 1 {
        return Err("地址里找不到 owner/repo".into());
    }
    Ok((owner.to_string(), repo.to_string()))
}

/// Fallback for non-GitHub remotes: clone with git, asking it to use the
/// Windows certificate store so an intercepting proxy does not break TLS.
async fn clone_repo(url: &str, proxy: &str, slug: &str) -> Result<ExtensionInfo, String> {
    let tmp = std::env::temp_dir().join(format!("irixen-ext-{}", crate::util::now_ms()));
    let mut cmd = std::process::Command::new("git");
    if let Some(p) = net::resolve_proxy(proxy) {
        cmd.arg("-c").arg(format!("http.proxy={p}"));
    }
    #[cfg(windows)]
    {
        cmd.arg("-c").arg("http.sslBackend=schannel");
    }
    cmd.args(["clone", "--depth", "1", "--quiet", url]).arg(&tmp);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let out = cmd.output().map_err(|e| format!("无法执行 git：{e}"))?;
    if !out.status.success() {
        let _ = std::fs::remove_dir_all(&tmp);
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!("克隆失败：{}", crate::util::one_line(&err, 300)));
    }
    let root = dir().join(sanitize(slug));
    if root.exists() {
        let _ = std::fs::remove_dir_all(&root);
    }
    std::fs::create_dir_all(&root).map_err(|e| format!("创建目录失败：{e}"))?;
    copy_tree(&tmp, &root)?;
    let _ = std::fs::remove_dir_all(&tmp);
    finish_repo(root, slug.to_string())
}

/// Shared tail: read package.json, find skills, write the sidecar.
fn finish_repo(root: PathBuf, slug: String) -> Result<ExtensionInfo, String> {
    let skills = find_skills(&root);
    let pkg: PackageJson = std::fs::read(root.join("package.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    let has_pkg = root.join("package.json").is_file();
    let kind = if has_pkg && skills.is_empty() { "vscode" } else { "dsh" };

    let nls = nls_map(&root, &ui_locale());
    let mut info = ExtensionInfo {
        id: slug.clone(),
        dir: root.to_string_lossy().to_string(),
        name: slug.clone(),
        publisher: slug.split('.').next().unwrap_or("local").to_string(),
        version: pkg.version.clone(),
        display_name: {
            let name = localize_str(&pkg.display_name, &nls);
            if name.is_empty() { slug.clone() } else { name }
        },
        description: localize_str(&pkg.description, &nls),
        icon: icon_path(&root, &pkg),
        kind: kind.into(),
        source: "git".into(),
        enabled: true,
        installed_at: crate::util::now_ms(),
        contributes: contributions(&pkg, &root, &ui_locale()),
        entry: pkg.main.trim_start_matches("./").replace('\\', "/"),
        activation: pkg.activation_events.clone(),
        broken: false,
        notice: ExtNotice::default(),
    };
    info.contributes.skills = skills;
    info.notice = notice_for(&info);
    write_meta(&root, &info)?;
    Ok(info)
}

impl ExtensionInfo {
    fn skill_count(&self) -> usize {
        self.contributes.skills.len()
    }
}

/// Import every `SKILL.md` of a plugin into the skill registry.
pub fn import_skills(info: &ExtensionInfo, registry: &crate::skills::SkillRegistry) -> Vec<String> {
    let mut imported = Vec::new();
    let base = PathBuf::from(&info.dir);
    for rel in &info.contributes.skills {
        let path = base.join(rel);
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let slug = rel
            .trim_end_matches("SKILL.md")
            .trim_end_matches('/')
            .replace(['/', '\\'], "-");
        let hint = format!("{}-{}", info.id, if slug.is_empty() { "main" } else { &slug });
        if registry.install_from_text(&hint, &text).is_ok() {
            imported.push(hint);
        }
    }
    imported
}

/// `SKILL.md` files inside a plugin tree (depth-limited, vendor dirs skipped).
fn find_skills(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        if depth > 4 || out.len() >= 40 {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if path.is_dir() {
                if matches!(name.as_str(), "node_modules" | ".git" | "target" | "dist") {
                    continue;
                }
                stack.push((path, depth + 1));
            } else if name.eq_ignore_ascii_case("SKILL.md") {
                if let Ok(rel) = path.strip_prefix(root) {
                    out.push(rel.to_string_lossy().replace('\\', "/"));
                }
            }
        }
    }
    out.sort();
    out
}

fn copy_tree(from: &Path, to: &Path) -> Result<(), String> {
    let mut files = 0usize;
    let mut total = 0u64;
    let mut stack = vec![from.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if name == ".git" {
                continue;
            }
            let Ok(rel) = path.strip_prefix(from) else {
                continue;
            };
            let Some(rel) = safe_rel(&rel.to_string_lossy().replace('\\', "/")) else {
                continue;
            };
            let target = to.join(&rel);
            if path.is_dir() {
                let _ = std::fs::create_dir_all(&target);
                stack.push(path);
            } else if path.is_file() {
                if files >= MAX_FILES || total > MAX_TOTAL_BYTES {
                    continue;
                }
                if let Some(parent) = target.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                if let Ok(meta) = path.metadata() {
                    total += meta.len();
                }
                if std::fs::copy(&path, &target).is_ok() {
                    files += 1;
                }
            }
        }
    }
    if files == 0 {
        return Err("仓库是空的".into());
    }
    Ok(())
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' { c } else { '-' })
        .collect()
}

/// Reject absolute paths, `..`, and drive-relative names from untrusted archives.
fn safe_rel(name: &str) -> Option<PathBuf> {
    if name.is_empty() || name.contains(':') {
        return None;
    }
    let p = PathBuf::from(name.replace('\\', "/"));
    for c in p.components() {
        match c {
            Component::Normal(_) => {}
            _ => return None,
        }
    }
    Some(p)
}

// ---------------------------------------------------------------------------
// search / themes / snippets
// ---------------------------------------------------------------------------

/// Open VSX search. `category` empty = all; `sort` is `downloads` | `rating`
/// | `recent`.
pub async fn search(
    query: &str,
    category: &str,
    sort: &str,
    offset: usize,
    proxy: &str,
) -> Result<Vec<ExtSearchRow>, String> {
    let q = query.trim();
    let sort_by = match sort {
        // Open VSX accepts `rating`, not `averageRating` (the latter is the
        // *response* field name). Sending it yields HTTP 400 and an empty list.
        "rating" => "rating",
        "recent" => "timestamp",
        _ => "downloadCount",
    };
    let mut url = format!(
        "{OPEN_VSX}/-/search?query={}&size=30&offset={}&sortBy={sort_by}&sortOrder=desc",
        urlencode(q),
        offset
    );
    if !category.trim().is_empty() && category != "all" {
        url.push_str(&format!("&category={}", urlencode(category)));
    }
    let client = net::build_client(proxy);
    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| net::explain_transport(&e, &url, &net::proxy_label(proxy)))?;
    if !resp.status().is_success() {
        return Err(format!("搜索失败（HTTP {}）", resp.status().as_u16()));
    }
    let body: serde_json::Value = resp.json().await.map_err(|e| format!("解析失败：{e}"))?;
    let installed: Vec<ExtensionInfo> = list();
    let rows = body
        .get("extensions")
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .map(|it| {
                    let namespace = str_field(it, "namespace");
                    let name = str_field(it, "name");
                    let id = format!("{namespace}.{name}");
                    ExtSearchRow {
                        installed_version: installed
                            .iter()
                            .find(|e| e.id.eq_ignore_ascii_case(&id))
                            .map(|e| e.version.clone())
                            .unwrap_or_default(),
                        id,
                        namespace,
                        name,
                        display_name: str_field(it, "displayName"),
                        description: crate::util::one_line(&str_field(it, "description"), 200),
                        version: str_field(it, "version"),
                        downloads: it.get("downloadCount").and_then(|v| v.as_i64()).unwrap_or(0),
                        rating: it.get("averageRating").and_then(|v| v.as_f64()).unwrap_or(0.0),
                        timestamp: str_field(it, "timestamp"),
                        icon: it
                            .get("files")
                            .and_then(|f| f.get("icon"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                    }
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Ok(rows)
}

/// Full metadata + README for one extension. The README is markdown from the
/// registry; the renderer escapes HTML, so no sanitizing is needed here.
/// Ask Open VSX for the newest version of every installed extension.
///
/// Runs once on startup, then again when the user hits refresh. Results live in
/// a process-local cache for ten minutes so opening the panel does not refetch.
/// A registry that is slow, offline or simply does not publish a given plugin
/// is not an error: the check is advisory, so failures are silently skipped.
static UPDATE_CACHE: OnceLock<Mutex<Option<(Instant, Vec<ExtUpdate>)>>> = OnceLock::new();

pub async fn check_updates(force: bool, proxy: &str) -> Vec<ExtUpdate> {
    let cache = UPDATE_CACHE.get_or_init(|| Mutex::new(None));
    if !force {
        if let Ok(guard) = cache.lock() {
            if let Some((at, rows)) = guard.as_ref() {
                if at.elapsed() < Duration::from_secs(600) {
                    return rows.clone();
                }
            }
        }
    }

    let candidates: Vec<ExtensionInfo> = list()
        .into_iter()
        .filter(|e| {
            !e.source.eq_ignore_ascii_case("git")
                && e.id.contains('.')
                && !e.version.trim().is_empty()
        })
        .collect();
    let client = net::build_client(proxy);
    let tasks = candidates.iter().map(|e| {
        let client = client.clone();
        let id = e.id.clone();
        let current = e.version.clone();
        async move {
            let (publisher, name) = id.split_once('.')?;
            let url = format!("{OPEN_VSX}/{publisher}/{name}/latest");
            let resp = client
                .get(&url)
                .timeout(Duration::from_secs(8))
                .send()
                .await
                .ok()?;
            if !resp.status().is_success() {
                return None;
            }
            let meta: serde_json::Value = resp.json().await.ok()?;
            let latest = str_field(&meta, "version");
            if latest.is_empty()
                || semver_cmp(&latest, &current) != std::cmp::Ordering::Greater
            {
                return None;
            }
            Some(ExtUpdate { id, current, latest })
        }
    });
    let found: Vec<ExtUpdate> = futures_util::future::join_all(tasks)
        .await
        .into_iter()
        .flatten()
        .collect();
    if let Ok(mut guard) = cache.lock() {
        *guard = Some((Instant::now(), found.clone()));
    }
    found
}

pub async fn detail(id: &str, proxy: &str) -> Result<ExtDetail, String> {
    let (publisher, name) = id
        .split_once('.')
        .ok_or_else(|| "需要 publisher.name 形式".to_string())?;
    let url = format!("{OPEN_VSX}/{publisher}/{name}");
    let client = net::build_client(proxy);
    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| net::explain_transport(&e, &url, &net::proxy_label(proxy)))?;
    if !resp.status().is_success() {
        return Err(format!("查询失败（HTTP {}）", resp.status().as_u16()));
    }
    let meta: serde_json::Value = resp.json().await.map_err(|e| format!("解析失败：{e}"))?;

    let mut versions: Vec<String> = meta
        .get("allVersions")
        .and_then(|v| v.as_object())
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default();
    versions.sort_by(|a, b| semver_cmp(b, a));
    if versions.is_empty() {
        let v = str_field(&meta, "version");
        if !v.is_empty() {
            versions.push(v);
        }
    }

    let file_url = |key: &str| -> String {
        meta.get("files")
            .and_then(|f| f.get(key))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    };
    let readme_url = file_url("readme");
    let mut readme = String::new();
    if !readme_url.is_empty() {
        if let Ok(r) = client.get(&readme_url).send().await {
            if r.status().is_success() {
                if let Ok(text) = r.text().await {
                    readme = crate::util::cap_chars(&text, 60_000);
                }
            }
        }
    }
    // `engines.vscode` is not the answer: even a pure icon theme declares it.
    // The registry serves the VSIX's package.json as `files.manifest`, which
    // has the one field that matters — `main` (the Node host entry point).
    let mut node = false;
    let manifest_url = file_url("manifest");
    if !manifest_url.is_empty() {
        if let Ok(r) = client.get(&manifest_url).send().await {
            if r.status().is_success() {
                if let Ok(pkg) = r.json::<serde_json::Value>().await {
                    node = pkg
                        .get("main")
                        .and_then(|v| v.as_str())
                        .map(|m| !m.trim().is_empty())
                        .unwrap_or(false);
                }
            }
        }
    }

    let installed = list()
        .into_iter()
        .find(|e| e.id.eq_ignore_ascii_case(id))
        .map(|e| e.version)
        .unwrap_or_default();

    Ok(ExtDetail {
        id: id.to_string(),
        namespace: str_field(&meta, "namespace"),
        name: str_field(&meta, "name"),
        display_name: str_field(&meta, "displayName"),
        description: str_field(&meta, "description"),
        version: str_field(&meta, "version"),
        versions,
        downloads: meta.get("downloadCount").and_then(|v| v.as_i64()).unwrap_or(0),
        rating: meta.get("averageRating").and_then(|v| v.as_f64()).unwrap_or(0.0),
        reviews: meta.get("reviewCount").and_then(|v| v.as_i64()).unwrap_or(0),
        license: str_field(&meta, "license"),
        repository: str_field(&meta, "repository"),
        homepage: str_field(&meta, "homepage"),
        categories: str_list(meta.get("categories")),
        readme,
        node,
        installed_version: installed,
    })
}

/// Loose semver compare: `1.10.0` > `1.9.9`. Falls back to string order.
fn semver_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let parse = |s: &str| -> Vec<u32> {
        s.split(['.', '-'])
            .map(|p| p.parse::<u32>().unwrap_or(0))
            .collect()
    };
    let (va, vb) = (parse(a), parse(b));
    for i in 0..va.len().max(vb.len()) {
        let x = va.get(i).copied().unwrap_or(0);
        let y = vb.get(i).copied().unwrap_or(0);
        if x != y {
            return x.cmp(&y);
        }
    }
    std::cmp::Ordering::Equal
}

/// Resolve a contributed theme to `{ "--bg-0": "#0b0f16", ... }`.
///
/// VS Code names 700+ workbench colours; we only own a dozen variables, so this
/// maps the handful that actually drive our layout and lets the rest go.
pub fn theme_colors(id: &str, label: &str) -> Result<serde_json::Value, String> {
    let (base, info) = find_dir(id)?;
    let theme = info
        .contributes
        .themes
        .iter()
        .find(|t| t.label == label || t.id == label)
        .ok_or_else(|| format!("扩展里没有主题 {label}"))?;
    let rel = theme.path.trim_start_matches("./");
    let path = base.join(rel);
    let raw = std::fs::read_to_string(&path).map_err(|e| format!("读取主题失败：{e}"))?;
    let json: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("主题不是合法 JSON：{e}"))?;
    let colors = json.get("colors").cloned().unwrap_or(serde_json::Value::Null);
    let get = |k: &str| colors.get(k).and_then(|v| v.as_str()).map(|s| s.to_string());

    let mut out = serde_json::Map::new();
    let mut set = |var: &str, keys: &[&str]| {
        for k in keys {
            if let Some(v) = get(k) {
                out.insert(var.to_string(), serde_json::Value::String(v));
                return;
            }
        }
    };
    set("--bg-0", &["editor.background", "editorGroup.background"]);
    set("--bg-1", &["sideBar.background", "activityBar.background"]);
    set("--bg-2", &["editorWidget.background", "panel.background", "button.secondaryBackground"]);
    set("--border", &["panel.border", "sideBar.border", "editorGroup.border", "contrastBorder"]);
    set("--text-0", &["editor.foreground", "foreground"]);
    set("--text-1", &["descriptionForeground", "sideBar.foreground"]);
    set("--text-2", &["disabledForeground", "tab.inactiveForeground"]);
    set("--accent", &["focusBorder", "button.background", "activityBarBadge.background"]);
    Ok(serde_json::Value::Object(out))
}

pub fn snippets_for(language: &str) -> Vec<SnippetRow> {
    let mut rows = Vec::new();
    for info in list() {
        if !info.enabled {
            continue;
        }
        let base = PathBuf::from(&info.dir);
        for snip in &info.contributes.snippets {
            if !snip.language.is_empty() && !same_lang(&snip.language, language) {
                continue;
            }
            let path = base.join(snip.path.trim_start_matches("./"));
            let Ok(raw) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Ok(json) = serde_json::from_str::<serde_json::Value>(&strip_jsonc(&raw)) else {
                continue;
            };
            let Some(map) = json.as_object() else {
                continue;
            };
            for (name, body) in map {
                if name.starts_with("//") {
                    continue;
                }
                let prefix = str_list(body.get("prefix")).join(",");
                let text = str_list(body.get("body")).join("\n");
                if prefix.is_empty() || text.is_empty() {
                    continue;
                }
                rows.push(SnippetRow {
                    ext: info.id.clone(),
                    language: snip.language.clone(),
                    prefix,
                    body: text,
                    description: str_field(body, "description"),
                });
                if rows.len() >= 400 {
                    return rows;
                }
            }
        }
    }
    rows
}

fn same_lang(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

/// VS Code snippet files allow `//` comments; `serde_json` does not.
fn strip_jsonc(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut in_str = false;
    let mut escaped = false;
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        if in_str {
            out.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        if c == '"' {
            in_str = true;
            out.push(c);
            continue;
        }
        if c == '/' {
            match chars.peek() {
                Some('/') => {
                    for n in chars.by_ref() {
                        if n == '\n' {
                            out.push('\n');
                            break;
                        }
                    }
                    continue;
                }
                Some('*') => {
                    chars.next();
                    let mut prev = '\0';
                    for n in chars.by_ref() {
                        if prev == '*' && n == '/' {
                            break;
                        }
                        prev = n;
                    }
                    continue;
                }
                _ => {}
            }
        }
        out.push(c);
    }
    out
}

fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// minimal ZIP reader
// ---------------------------------------------------------------------------

pub(crate) mod zip {
    use super::*;

    pub struct Entry {
        pub name: String,
        method: u16,
        comp_size: u64,
        uncomp_size: u64,
        local_offset: u64,
    }

    fn u16_at(b: &[u8], i: usize) -> u16 {
        u16::from_le_bytes([b[i], b[i + 1]])
    }

    fn u32_at(b: &[u8], i: usize) -> u32 {
        u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
    }

    fn u64_at(b: &[u8], i: usize) -> u64 {
        let mut v = [0u8; 8];
        v.copy_from_slice(&b[i..i + 8]);
        u64::from_le_bytes(v)
    }

    /// Large packages (the Java/C++ toolchains routinely are) spill past the
    /// 32-bit fields of the classic end-of-central-directory record and switch
    /// to ZIP64. Reading only the 32-bit fields there yields a count of 0xFFFF
    /// and an offset of 0xFFFFFFFF — every entry then fails its signature check
    /// and the package unpacks to a nearly empty directory.
    fn zip64_eocd(bytes: &[u8], eocd: usize) -> Option<(usize, usize, usize)> {
        // The locator sits immediately before the classic record.
        let loc = eocd.checked_sub(20)?;
        if u32_at(bytes, loc) != 0x0706_4b50 {
            return None;
        }
        let off = u64_at(bytes, loc + 8) as usize;
        if off + 56 > bytes.len() || u32_at(bytes, off) != 0x0606_4b50 {
            return None;
        }
        let count = u64_at(bytes, off + 32) as usize;
        let size = u64_at(bytes, off + 40) as usize;
        let start = u64_at(bytes, off + 48) as usize;
        Some((count, size, start))
    }

    /// Walk the central directory.
    pub fn list(bytes: &[u8]) -> Result<Vec<Entry>, String> {
        if bytes.len() < 22 {
            return Err("文件太小，不是 zip".into());
        }
        let tail_start = bytes.len().saturating_sub(66_000);
        let eocd = (tail_start..bytes.len().saturating_sub(3))
            .rev()
            .find(|&i| bytes[i] == 0x50 && bytes[i + 1] == 0x4b && bytes[i + 2] == 0x05 && bytes[i + 3] == 0x06)
            .ok_or_else(|| "找不到 zip 目录尾，文件可能已损坏".to_string())?;

        let mut count = u16_at(bytes, eocd + 10) as usize;
        let mut cd_size = u32_at(bytes, eocd + 12) as usize;
        let mut cd_off = u32_at(bytes, eocd + 16) as usize;
        let wide = count == 0xFFFF || cd_off == 0xFFFF_FFFF || cd_size == 0xFFFF_FFFF;
        if wide {
            if let Some((c, s, o)) = zip64_eocd(bytes, eocd) {
                count = c;
                cd_size = s;
                cd_off = o;
            }
        }
        if cd_off.saturating_add(cd_size) > bytes.len() {
            return Err("zip 目录越界".into());
        }

        let cd = &bytes[cd_off..cd_off + cd_size];
        let mut entries = Vec::new();
        let mut p = 0usize;
        for _ in 0..count {
            if p + 46 > cd.len() || u32_at(cd, p) != 0x0201_4b50 {
                break;
            }
            let method = u16_at(cd, p + 10);
            let mut comp_size = u32_at(cd, p + 20) as u64;
            let mut uncomp_size = u32_at(cd, p + 24) as u64;
            let name_len = u16_at(cd, p + 28) as usize;
            let extra_len = u16_at(cd, p + 30) as usize;
            let comment_len = u16_at(cd, p + 32) as usize;
            let mut local_offset = u32_at(cd, p + 42) as u64;
            if p + 46 + name_len > cd.len() {
                break;
            }
            let name = String::from_utf8_lossy(&cd[p + 46..p + 46 + name_len]).to_string();

            // ZIP64 moves any 32-bit field that overflowed into an extra block,
            // in the fixed order uncompressed → compressed → offset.
            if extra_len > 0 {
                let ex = &cd[p + 46 + name_len..(p + 46 + name_len + extra_len).min(cd.len())];
                let mut q = 0usize;
                while q + 4 <= ex.len() {
                    let id = u16_at(ex, q);
                    let sz = u16_at(ex, q + 2) as usize;
                    let body = &ex[(q + 4).min(ex.len())..(q + 4 + sz).min(ex.len())];
                    if id == 0x0001 {
                        let mut r = 0usize;
                        if uncomp_size == 0xFFFF_FFFF && r + 8 <= body.len() {
                            uncomp_size = u64_at(body, r);
                            r += 8;
                        }
                        if comp_size == 0xFFFF_FFFF && r + 8 <= body.len() {
                            comp_size = u64_at(body, r);
                            r += 8;
                        }
                        if local_offset == 0xFFFF_FFFF && r + 8 <= body.len() {
                            local_offset = u64_at(body, r);
                        }
                        break;
                    }
                    q += 4 + sz;
                }
            }

            entries.push(Entry {
                name,
                method,
                comp_size,
                uncomp_size,
                local_offset,
            });
            p += 46 + name_len + extra_len + comment_len;
        }
        if entries.is_empty() {
            return Err("zip 里没有条目".into());
        }
        Ok(entries)
    }

    pub fn read(bytes: &[u8], e: &Entry) -> Result<Vec<u8>, String> {
        let off = e.local_offset as usize;
        if off + 30 > bytes.len() {
            return Err("条目偏移越界".into());
        }
        let name_len = u16_at(bytes, off + 26) as usize;
        let extra_len = u16_at(bytes, off + 28) as usize;
        let data_start = off + 30 + name_len + extra_len;
        let data_end = data_start + e.comp_size as usize;
        if data_end > bytes.len() {
            return Err("条目数据越界".into());
        }
        let data = &bytes[data_start..data_end];
        match e.method {
            0 => Ok(data.to_vec()),
            8 => {
                let mut out = Vec::with_capacity(e.uncomp_size as usize);
                let mut dec = flate2::read::DeflateDecoder::new(data);
                dec.read_to_end(&mut out).map_err(|err| format!("解压失败：{err}"))?;
                Ok(out)
            }
            m => Err(format!("不支持的压缩方式 {m}")),
        }
    }
}


// ---------------------------------------------------------------------------
// installed-extension detail (fully offline)
// ---------------------------------------------------------------------------

/// Everything the installed-extension detail pane needs. `ExtDetail` above is
/// an Open VSX *store* lookup; this one is read from the local install only, so
/// it works with no network and also covers git / .vsix plugins the registry
/// has never heard of.
#[derive(Serialize, Clone, Default)]
#[serde(default)]
pub struct ExtInspect {
    pub info: ExtensionInfo,
    pub readme: String,
    pub changelog: String,
    pub license: String,
    pub license_text: String,
    pub homepage: String,
    pub repository: String,
    pub categories: Vec<String>,
    pub keywords: Vec<String>,
    pub engines: String,
    pub size_bytes: u64,
    pub file_count: usize,
}

const MAX_DOC_BYTES: usize = 96 * 1024;

fn json_strings(v: Option<&serde_json::Value>) -> Vec<String> {
    v.and_then(|x| x.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn url_field(v: Option<&serde_json::Value>) -> String {
    match v {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(obj) => obj
            .get("url")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
        None => String::new(),
    }
}

/// Read one of several possible doc filenames from the extension root. Windows
/// is case-insensitive but Linux is not, so scan the directory instead of
/// assuming `README.md` is spelled exactly that way.
fn read_doc(root: &Path, names: &[&str]) -> String {
    let mut candidate: Option<PathBuf> = None;
    if let Ok(entries) = std::fs::read_dir(root) {
        for e in entries.flatten() {
            let p = e.path();
            if !p.is_file() {
                continue;
            }
            let Some(n) = p.file_name().and_then(|x| x.to_str()) else {
                continue;
            };
            if names.iter().any(|wanted| n.eq_ignore_ascii_case(wanted)) {
                candidate = Some(p);
                break;
            }
        }
    }
    let Some(p) = candidate else {
        return String::new();
    };
    let mut bytes = std::fs::read(&p).unwrap_or_default();
    bytes.truncate(MAX_DOC_BYTES);
    String::from_utf8_lossy(&bytes).into_owned()
}

fn dir_stats(root: &Path) -> (u64, usize) {
    fn walk(p: &Path, bytes: &mut u64, files: &mut usize, depth: usize) {
        if depth > 8 || *files > 20_000 {
            return;
        }
        let Ok(rd) = std::fs::read_dir(p) else { return };
        for e in rd.flatten() {
            let path = e.path();
            if path.is_dir() {
                walk(&path, bytes, files, depth + 1);
            } else if let Ok(m) = e.metadata() {
                *bytes = bytes.saturating_add(m.len());
                *files += 1;
            }
        }
    }
    let mut out = (0u64, 0usize);
    walk(root, &mut out.0, &mut out.1, 0);
    out
}

pub fn inspect(id: &str) -> Result<ExtInspect, String> {
    let (root, info) = find_dir(id)?;
    let pkg: serde_json::Value = std::fs::read(root.join("package.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_else(|| serde_json::Value::Object(Default::default()));

    let engines = match pkg.get("engines") {
        Some(v) if v.is_object() => v
            .as_object()
            .map(|m| {
                m.iter()
                    .map(|(k, x)| format!("{k}: {}", x.as_str().unwrap_or("")))
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default(),
        Some(v) => v.to_string(),
        None => String::new(),
    };
    let license_text = read_doc(
        &root,
        &["LICENSE", "LICENSE.md", "LICENSE.txt", "LICENCE", "COPYING"],
    );
    let declared_license = str_field(&pkg, "license");
    let (size_bytes, file_count) = dir_stats(&root);

    Ok(ExtInspect {
        info,
        readme: read_doc(&root, &["README.md", "README", "readme.txt"]),
        changelog: read_doc(&root, &["CHANGELOG.md", "CHANGELOG.txt", "CHANGES.md"]),
        license: if declared_license.is_empty() {
            license_text.lines().next().unwrap_or("").trim().to_string()
        } else {
            declared_license
        },
        license_text,
        homepage: str_field(&pkg, "homepage"),
        repository: url_field(pkg.get("repository")),
        categories: json_strings(pkg.get("categories")),
        keywords: json_strings(pkg.get("keywords")),
        engines,
        size_bytes,
        file_count,
    })
}

#[cfg(test)]
mod l10n_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn resolves_nls_keys_and_locale_fallback() {
        let dir = std::env::temp_dir().join("irixen-nls-test");
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(
            dir.join("package.nls.json"),
            r#"{"commands.start":"Start","commands.stop":"Stop"}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("package.nls.zh-cn.json"),
            r#"{"commands.start":"启动"}"#,
        )
        .unwrap();

        let manifest = json!({
            "displayName": "%commands.start%",
            "contributes": {
                "commands": [
                    { "command": "x.start", "title": "%commands.start%", "category": "%commands.stop%" },
                    { "command": "x.plain", "title": "Plain", "category": "" }
                ]
            }
        });

        let zh = localize_manifest(&manifest, &dir, "zh-CN");
        assert_eq!(zh["displayName"], "启动");
        assert_eq!(zh["contributes"]["commands"][0]["title"], "启动");
        // Untranslated keys fall back to the base file, never to `%key%`.
        assert_eq!(zh["contributes"]["commands"][0]["category"], "Stop");
        assert_eq!(zh["contributes"]["commands"][1]["title"], "Plain");

        let en = localize_manifest(&manifest, &dir, "en-US");
        assert_eq!(en["contributes"]["commands"][0]["title"], "Start");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn collapses_localized_string_objects() {
        let dir = std::env::temp_dir().join("irixen-nls-obj");
        let _ = std::fs::create_dir_all(&dir);
        let manifest = json!({ "title": { "value": "保存", "original": "Save" } });
        let out = localize_manifest(&manifest, &dir, "zh-CN");
        assert_eq!(out["title"], "保存");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
