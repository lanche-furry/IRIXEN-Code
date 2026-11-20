//! Deterministic security triage behind the `security_scan` tool.
//!
//! Division of labour: this module enumerates, the model judges. A language
//! model is bad at sweeping a tree consistently and good at deciding whether a
//! hit is actually reachable — so the rules here are dumb, fixed and repeatable
//! (no AST, no network, no heuristics that change between runs), and every hit
//! goes to the model as `path:line` plus a hint.
//!
//! Two invariants worth keeping:
//!
//! 1. **Secret material never leaves in the clear.** Any rule marked `secret`
//!    has its matched span replaced by a mask before the evidence string is
//!    built. An "AI security audit" that ships an AWS key into a prompt is a
//!    worse outcome than not auditing at all.
//! 2. **A clean scan is not a pass.** The report says so in as many words,
//!    because a rule table only covers what someone thought to write down.

use ignore::WalkBuilder;
use std::path::Path;

const MAX_FILE_BYTES: u64 = 1_500_000;
const MAX_FILES: usize = 6000;
const BINARY_SNIFF: usize = 8192;
const MAX_PER_RULE_PER_FILE: usize = 3;
const EVIDENCE_CHARS: usize = 160;

/// Directories that are never interesting and can be enormous.
const SKIP_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "dist",
    "build",
    "vendor",
    ".venv",
    "venv",
    "__pycache__",
    ".next",
    ".nuxt",
    ".svelte-kit",
    "coverage",
    ".cache",
    ".gradle",
    ".terraform",
    "Pods",
];

/// Extensions that cannot contain reviewable source text.
const SKIP_EXT: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "ico", "bmp", "svgz", "pdf", "zip", "gz", "bz2", "xz",
    "7z", "tar", "exe", "dll", "so", "dylib", "a", "lib", "o", "obj", "wasm", "class", "jar",
    "woff", "woff2", "ttf", "otf", "eot", "mp3", "mp4", "mov", "wav", "webm", "sqlite", "db",
];

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Severity {
    High,
    Medium,
    Low,
}

impl Severity {
    fn label(self) -> &'static str {
        match self {
            Severity::High => "HIGH",
            Severity::Medium => "MED",
            Severity::Low => "LOW",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Finding {
    pub rule: &'static str,
    pub what: &'static str,
    pub severity: Severity,
    pub path: String,
    pub line: usize,
    pub evidence: String,
    pub hint: &'static str,
}

#[derive(Clone, Copy, Default)]
pub enum Scope {
    #[default]
    Workspace,
    /// Only files reported by `git status` — the cheap, high-signal default.
    Changed,
    /// One file or directory inside the workspace.
    Path,
}

impl Scope {
    pub fn parse(raw: &str) -> Scope {
        match raw.trim().to_ascii_lowercase().as_str() {
            "changed" | "diff" | "staged" => Scope::Changed,
            "path" | "file" | "dir" => Scope::Path,
            _ => Scope::Workspace,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Scope::Workspace => "workspace",
            Scope::Changed => "changed",
            Scope::Path => "path",
        }
    }
}

pub struct ScanOptions {
    pub scope: Scope,
    pub path: Option<String>,
    pub max_findings: usize,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            scope: Scope::Workspace,
            path: None,
            max_findings: 80,
        }
    }
}

#[derive(Default)]
pub struct Report {
    pub findings: Vec<Finding>,
    pub files_read: usize,
    pub files_skipped: usize,
    pub truncated: bool,
    scope: &'static str,
}

/// A byte range on one line that has to be masked before it is quoted.
#[derive(Clone, Copy, Debug)]
struct Hit {
    start: usize,
    end: usize,
}

impl Hit {
    fn none() -> Option<Hit> {
        Some(Hit { start: 0, end: 0 })
    }
}

struct Rule {
    id: &'static str,
    what: &'static str,
    severity: Severity,
    hint: &'static str,
    /// Cheap gates: the lowercased line must contain one of these before the
    /// matcher runs. Keeps a 600k-line sweep in the millisecond range.
    gates: &'static [&'static str],
    /// Mask the matched span. Never quote secret material verbatim.
    secret: bool,
    scan: fn(&str) -> Option<Hit>,
}

const RULES: &[Rule] = &[
    Rule {
        id: "private-key",
        what: "private key block",
        severity: Severity::High,
        hint: "rotate the key, delete it from the tree, and purge it from history",
        gates: &["private key-----"],
        secret: true,
        scan: |line| {
            if line.contains("-----BEGIN") {
                Hit::none()
            } else {
                None
            }
        },
    },
    Rule {
        id: "cloud-credential",
        what: "cloud credential",
        severity: Severity::High,
        hint: "rotate it, read it from the environment at runtime, and check whether it is still live",
        gates: &[
            "akia", "asia", "ghp_", "gho_", "ghu_", "ghs_", "ghr_", "github_pat_", "xoxb-",
            "xoxp-", "xoxa-", "xoxr-", "sk_live_", "rk_live_", "sk-ant-", "sk-proj-", "aiza",
            "glpat-",
        ],
        secret: true,
        scan: scan_cloud_credential,
    },
    Rule {
        id: "jwt",
        what: "JSON web token",
        severity: Severity::Medium,
        hint: "tokens in source are long-lived: move to runtime config and set a short expiry",
        gates: &["eyj"],
        secret: true,
        scan: scan_jwt,
    },
    Rule {
        id: "hardcoded-secret",
        what: "secret assigned a literal value",
        severity: Severity::High,
        hint: "read it from config or env; if it ever shipped, rotate it",
        gates: &[
            "password",
            "passwd",
            "secret",
            "api_key",
            "api-key",
            "apikey",
            "access_key",
            "auth_token",
            "private_key",
            "client_secret",
        ],
        secret: true,
        scan: scan_assigned_secret,
    },
    Rule {
        id: "tls-verify-off",
        what: "certificate verification disabled",
        severity: Severity::High,
        hint: "this makes every TLS connection on the path MITM-able; pin the CA instead",
        gates: &[
            "accept_invalid_certs",
            "insecure_skip_verify",
            "rejectunauthorized: false",
            "verify=false",
            "verify: false",
            "tls_reject_unauthorized",
        ],
        secret: false,
        scan: |_| Hit::none(),
    },
    Rule {
        id: "dynamic-exec",
        what: "dynamic code or shell execution",
        severity: Severity::Medium,
        hint: "check whether any argument is attacker-influenced; prefer an argv array over a shell string",
        gates: &[
            "os.system(",
            "subprocess.popen(",
            "shell=true",
            "runtime.getruntime().exec(",
            "child_process.exec(",
            "execsync(",
            "eval(",
        ],
        secret: false,
        scan: |_| Hit::none(),
    },
    Rule {
        id: "sql-interpolation",
        what: "SQL built by string interpolation",
        severity: Severity::Medium,
        hint: "bind parameters instead of formatting values into the statement",
        gates: &["select ", "insert into", "update ", "delete from"],
        secret: false,
        scan: scan_sql_interpolation,
    },
    Rule {
        id: "world-writable",
        what: "world-writable permissions",
        severity: Severity::Low,
        hint: "0777 lets any local user rewrite the file; use 0600/0644 or an explicit umask",
        gates: &["chmod 777", "0o777", "mode=0o777", "0777"],
        secret: false,
        scan: |_| Hit::none(),
    },
];

/// File names that are almost always a live credential.
fn secret_file(name: &str) -> Option<&'static str> {
    let lower = name.to_ascii_lowercase();
    // Samples are the whole point of `.env.example` — do not flag them.
    for ok in [".example", ".sample", ".template", ".dist", ".md"] {
        if lower.ends_with(ok) {
            return None;
        }
    }
    if lower == ".env" || lower.starts_with(".env.") {
        return Some("environment file with credentials");
    }
    if lower == "id_rsa" || lower == "id_ed25519" || lower == "id_dsa" || lower == "id_ecdsa" {
        return Some("SSH private key");
    }
    if lower == "credentials.json" || lower.starts_with("service-account") {
        return Some("service account credentials");
    }
    for ext in [".pem", ".key", ".pfx", ".p12", ".keystore", ".jks"] {
        if lower.ends_with(ext) {
            return Some("key material");
        }
    }
    None
}

fn is_token(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-'
}

fn find_prefixed(line: &str, prefix: &str, need: usize) -> Option<Hit> {
    let mut from = 0usize;
    while from < line.len() {
        let rel = line[from..].find(prefix)?;
        let start = from + rel;
        let after = start + prefix.len();
        let rest = &line[after..];
        let mut n = 0usize;
        for b in rest.bytes() {
            if !is_token(b) {
                break;
            }
            n += 1;
            if n == need {
                break;
            }
        }
        if n >= need {
            return Some(Hit {
                start,
                end: after + need,
            });
        }
        from = after;
    }
    None
}

fn scan_cloud_credential(line: &str) -> Option<Hit> {
    // Prefix -> how many token characters must follow for it to be real.
    const PREFIXES: &[(&str, usize)] = &[
        ("AKIA", 16),
        ("ASIA", 16),
        ("ghp_", 36),
        ("gho_", 36),
        ("ghu_", 36),
        ("ghs_", 36),
        ("ghr_", 36),
        ("github_pat_", 22),
        ("xoxb-", 10),
        ("xoxp-", 10),
        ("xoxa-", 10),
        ("xoxr-", 10),
        ("sk_live_", 24),
        ("rk_live_", 24),
        ("sk-ant-", 20),
        ("sk-proj-", 20),
        ("AIza", 35),
        ("glpat-", 20),
    ];
    for (prefix, need) in PREFIXES {
        if let Some(h) = find_prefixed(line, prefix, *need) {
            return Some(h);
        }
    }
    None
}

fn scan_jwt(line: &str) -> Option<Hit> {
    for (start, _) in line.match_indices("eyJ") {
        let rest = &line[start..];
        let len = rest
            .bytes()
            .take_while(|b| is_token(*b) || *b == b'.')
            .count();
        if len < 50 {
            continue;
        }
        let token = &rest[..len];
        let segs: Vec<&str> = token.split('.').collect();
        if segs.len() == 3 && segs.iter().all(|s| s.len() >= 8) {
            return Some(Hit {
                start,
                end: start + len,
            });
        }
    }
    None
}

const SECRET_KEYS: &[&str] = &[
    "password",
    "passwd",
    "secret",
    "api_key",
    "api-key",
    "apikey",
    "access_key",
    "auth_token",
    "private_key",
    "client_secret",
    "token",
];

/// A key followed by one of these suffixes names metadata, not a value:
/// `password_hash`, `token_count`, `secret_name` are not secrets.
const IDENT_SUFFIX_SKIP: &[&str] = &[
    "_hash", "hash", "_len", "length", "_count", "count", "_name", "name", "_type", "type",
    "_policy", "policy", "_min", "_max", "_size", "_bytes", "_url", "url", "_path", "_file",
    "_hint", "_label", "_ttl", "_expiry", "_prefix", "_timeout", "_required",
];

fn scan_assigned_secret(line: &str) -> Option<Hit> {
    let lower = line.to_ascii_lowercase();
    for key in SECRET_KEYS {
        let mut from = 0usize;
        while from < lower.len() {
            let rel = match lower[from..].find(key) {
                Some(v) => v,
                None => break,
            };
            let start = from + rel;
            // Walk to the end of the whole identifier: `secretKey` is the same
            // thing as `secret_key`, and stopping at the prefix would miss it.
            let mut end = start + key.len();
            end += lower[end..].bytes().take_while(|b| is_token(*b)).count();
            let ident = &lower[start..end];
            let metadata = IDENT_SUFFIX_SKIP.iter().any(|s| ident.ends_with(s));
            if !metadata {
                if let Some(h) = value_after(line, end) {
                    return Some(h);
                }
            }
            from = start + key.len();
        }
    }
    None
}

/// Starting after a key name, find `= "literal"` / `: 'literal'` / `KEY=literal`.
fn value_after(line: &str, pos: usize) -> Option<Hit> {
    let bytes = line.as_bytes();
    let mut i = pos;
    while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
        i += 1;
    }
    // JSON/YAML allow a quoted key: `"password": "..."`.
    if i < bytes.len() && (bytes[i] == b'"' || bytes[i] == b'\'') {
        i += 1;
    }
    while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
        i += 1;
    }
    if i >= bytes.len() || (bytes[i] != b'=' && bytes[i] != b':') {
        return None;
    }
    i += 1;
    // `password == other` is a comparison, not an assignment.
    if i < bytes.len() && bytes[i] == b'=' {
        return None;
    }
    while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
        i += 1;
    }
    if i >= bytes.len() {
        return None;
    }
    let quote = bytes[i];
    if quote == b'"' || quote == b'\'' {
        let start = i + 1;
        let rest = &line[start..];
        let end = rest.find(quote as char)?;
        let value = &rest[..end];
        if value.len() < 8 || is_placeholder(value) {
            return None;
        }
        return Some(Hit {
            start,
            end: start + end,
        });
    }
    let len = line[i..]
        .bytes()
        .take_while(|b| is_token(*b) || *b == b'.' || *b == b'/')
        .count();
    if len < 12 {
        return None;
    }
    let value = &line[i..i + len];
    if is_placeholder(value) {
        return None;
    }
    Some(Hit {
        start: i,
        end: i + len,
    })
}

fn is_placeholder(value: &str) -> bool {
    const MARKS: &[&str] = &[
        "changeme",
        "change_me",
        "change-me",
        "placeholder",
        "example",
        "sample",
        "dummy",
        "redacted",
        "your_",
        "your-",
        "not-a-secret",
        "notasecret",
        "test_key",
        "test-key",
        "testkey",
        "fake",
        "todo",
        // Interpolation and format strings are not literals.
        "${",
        "{{",
        "%s",
        "<",
        ">",
        "...",
    ];
    let lower = value.to_ascii_lowercase();
    if MARKS.iter().any(|m| lower.contains(m)) {
        return true;
    }
    // `********` or `xxxxxxxx` — a redaction, not a value.
    value.chars().all(|c| c == '*' || c == 'x' || c == 'X' || c == '.')
}

fn scan_sql_interpolation(line: &str) -> Option<Hit> {
    let lower = line.to_ascii_lowercase();
    let sql = lower.contains("select ")
        || lower.contains("insert into")
        || lower.contains("update ")
        || lower.contains("delete from");
    if !sql {
        return None;
    }
    let interpolated = line.contains("format!(")
        || line.contains("\" +")
        || line.contains("' +")
        || line.contains("${")
        || line.contains("f\"")
        || line.contains("f'");
    if interpolated {
        Hit::none()
    } else {
        None
    }
}

/// `uses: owner/action@main` in a workflow file: whatever `main` points at
/// changes under you, which is a supply-chain hole rather than a bug.
fn scan_unpinned_action(line: &str) -> Option<&'static str> {
    let trimmed = line.trim_start();
    // YAML list item: `- uses: actions/checkout@v4`
    let trimmed = trimmed.strip_prefix("- ").unwrap_or(trimmed).trim_start();
    let rest = trimmed.strip_prefix("uses:")?;
    let action = rest.trim();
    if action.is_empty() || action.starts_with("./") || action.starts_with("docker://") {
        return None;
    }
    match action.rsplit_once('@') {
        None => Some("action used without a pinned ref"),
        Some((_, r)) => {
            let r = r.split_whitespace().next().unwrap_or("");
            if r == "main" || r == "master" || r == "HEAD" {
                Some("action pinned to a moving branch")
            } else if r.len() == 40 && r.chars().all(|c| c.is_ascii_hexdigit()) {
                None
            } else {
                None
            }
        }
    }
}

fn mask(line: &str, hit: Hit) -> String {
    let mut out = String::new();
    if hit.start == 0 && hit.end == 0 {
        out.push_str(line.trim());
    } else {
        let start = hit.start.min(line.len());
        let end = hit.end.min(line.len()).max(start);
        out.push_str(&line[..start]);
        let secret = &line[start..end];
        let head: String = secret.chars().take(4).collect();
        let tail: String = secret
            .chars()
            .rev()
            .take(3)
            .collect::<Vec<char>>()
            .into_iter()
            .rev()
            .collect();
        if secret.chars().count() > 12 {
            out.push_str(&format!("{head}***{tail}"));
        } else {
            out.push_str("***");
        }
        out.push_str(&line[end..]);
    }
    crate::util::one_line(out.trim(), EVIDENCE_CHARS)
}

fn rel_path(workspace: &Path, path: &Path) -> String {
    path.strip_prefix(workspace)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn is_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(BINARY_SNIFF).any(|b| *b == 0)
}

fn should_skip(workspace: &Path, path: &Path) -> bool {
    let rel = path.strip_prefix(workspace).unwrap_or(path);
    if rel.components().any(|c| {
        let name = c.as_os_str().to_string_lossy();
        SKIP_DIRS.iter().any(|s| name.eq_ignore_ascii_case(s))
    }) {
        return true;
    }
    match path.extension().and_then(|e| e.to_str()) {
        Some(ext) => {
            let lower = ext.to_ascii_lowercase();
            SKIP_EXT.contains(&lower.as_str())
        }
        None => false,
    }
}

/// Inline allowlist, the one thing every scanner needs and nobody adds first.
///
/// ```text
/// let key = "AKIA...";   // irixen-audit: ignore
/// dbg!("...");           // irixen-audit: ignore=cloud-credential,jwt
/// ```
///
/// Returns `Some(vec![])` for "suppress everything on this line", or the list
/// of rule ids to suppress.
fn suppression(lower: &str) -> Option<Vec<String>> {
    const MARK: &str = "irixen-audit: ignore";
    let at = lower.find(MARK)?;
    let rest = lower[at + MARK.len()..].trim_start();
    let Some(list) = rest.strip_prefix('=') else {
        return Some(Vec::new());
    };
    let taken: String = list
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ',' | ' '))
        .collect();
    Some(
        taken
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
    )
}

fn examine(file: &Path, workspace: &Path, report: &mut Report, max: usize) {
    let rel = rel_path(workspace, file);
    let name = file
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();

    if let Some(what) = secret_file(&name) {
        report.findings.push(Finding {
            rule: "secret-file",
            what,
            severity: Severity::High,
            path: rel.clone(),
            line: 1,
            evidence: format!("{name} is tracked in the workspace"),
            hint: "delete it from the tree, add the pattern to .gitignore, and rotate whatever it held",
        });
    }

    let meta = match std::fs::metadata(file) {
        Ok(m) => m,
        Err(_) => return,
    };
    if !meta.is_file() || meta.len() > MAX_FILE_BYTES {
        report.files_skipped += 1;
        return;
    }
    let bytes = match std::fs::read(file) {
        Ok(b) => b,
        Err(_) => {
            report.files_skipped += 1;
            return;
        }
    };
    if is_binary(&bytes) {
        report.files_skipped += 1;
        return;
    }
    report.files_read += 1;

    let text = String::from_utf8_lossy(&bytes);
    let in_workflows = rel.starts_with(".github/workflows/");
    let mut per_rule: Vec<(&'static str, usize)> = Vec::new();

    for (idx, raw) in text.lines().enumerate() {
        let line = raw.trim_end();
        if line.is_empty() || line.len() > 4000 {
            continue;
        }
        let lower = line.to_ascii_lowercase();
        let suppressed = suppression(&lower);
        let is_suppressed = |id: &str| match &suppressed {
            None => false,
            Some(ids) => ids.is_empty() || ids.iter().any(|i| i == id),
        };

        for rule in RULES {
            if !rule.gates.iter().any(|g| lower.contains(g)) {
                continue;
            }
            if is_suppressed(rule.id) {
                continue;
            }
            let hit = match (rule.scan)(line) {
                Some(h) => h,
                None => continue,
            };
            let count = per_rule
                .iter_mut()
                .find(|(id, _)| *id == rule.id)
                .map(|slot| {
                    slot.1 += 1;
                    slot.1
                })
                .unwrap_or_else(|| {
                    per_rule.push((rule.id, 1));
                    1
                });
            if count > MAX_PER_RULE_PER_FILE {
                continue;
            }
            report.findings.push(Finding {
                rule: rule.id,
                what: rule.what,
                severity: rule.severity,
                path: rel.clone(),
                line: idx + 1,
                evidence: if rule.secret {
                    mask(line, hit)
                } else {
                    crate::util::one_line(line.trim(), EVIDENCE_CHARS)
                },
                hint: rule.hint,
            });
            if report.findings.len() >= max {
                report.truncated = true;
                return;
            }
        }

        if in_workflows && !is_suppressed("unpinned-action") {
            if let Some(what) = scan_unpinned_action(line) {
                report.findings.push(Finding {
                    rule: "unpinned-action",
                    what,
                    severity: Severity::Low,
                    path: rel.clone(),
                    line: idx + 1,
                    evidence: crate::util::one_line(line.trim(), EVIDENCE_CHARS),
                    hint: "pin the full commit SHA so the code cannot change under a tag",
                });
                if report.findings.len() >= max {
                    report.truncated = true;
                    return;
                }
            }
        }
    }
}

fn walk_into(root: &Path, workspace: &Path, report: &mut Report, max: usize) {
    let mut builder = WalkBuilder::new(root);
    builder
        // Hidden files matter here: `.env` and `.github/workflows` are hidden
        // and are exactly what an audit is for.
        .hidden(false)
        .git_ignore(true)
        .git_global(false)
        .git_exclude(true)
        .parents(true)
        .max_depth(Some(24))
        // Prune whole subtrees rather than rejecting their files one by one:
        // walking node_modules to reject everything in it costs more than the
        // scan itself.
        .filter_entry(|e| {
            let name = e.file_name().to_string_lossy();
            !SKIP_DIRS.iter().any(|s| name.eq_ignore_ascii_case(s))
        });
    for entry in builder.build().flatten() {
        if report.findings.len() >= max {
            report.truncated = true;
            return;
        }
        let path = entry.path();
        let is_file = entry.file_type().map(|t| t.is_file()).unwrap_or(false);
        if !is_file {
            continue;
        }
        if should_skip(workspace, path) {
            report.files_skipped += 1;
            continue;
        }
        if report.files_read + report.files_skipped >= MAX_FILES {
            report.truncated = true;
            return;
        }
        examine(path, workspace, report, max);
    }
}

pub fn scan(workspace: &Path, opts: &ScanOptions) -> Result<Report, String> {
    if workspace.as_os_str().is_empty() {
        return Err("no workspace is open".into());
    }
    let workspace = workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.to_path_buf());
    let max = opts.max_findings.clamp(1, 300);
    let mut report = Report {
        scope: opts.scope.label(),
        ..Default::default()
    };

    match opts.scope {
        Scope::Workspace => walk_into(&workspace, &workspace, &mut report, max),
        Scope::Path => {
            let raw = opts.path.clone().unwrap_or_default();
            if raw.trim().is_empty() {
                return Err("scope=path needs a 'path' argument".into());
            }
            let target = workspace.join(raw.trim());
            let target = target
                .canonicalize()
                .map_err(|e| format!("cannot open {raw}: {e}"))?;
            if !target.starts_with(&workspace) {
                return Err(format!("refused: '{raw}' resolves outside the workspace"));
            }
            if target.is_dir() {
                walk_into(&target, &workspace, &mut report, max);
            } else {
                examine(&target, &workspace, &mut report, max);
            }
        }
        Scope::Changed => {
            let files = crate::git::status(&workspace)
                .map_err(|e| format!("scope=changed needs a git repository: {e}"))?;
            if files.is_empty() {
                return Err(
                    "no changed files. Use scope=workspace to audit the whole tree".into(),
                );
            }
            for f in files {
                if report.findings.len() >= max {
                    report.truncated = true;
                    break;
                }
                // Deleted files have nothing left to read.
                if f.code.contains('D') {
                    continue;
                }
                let p = workspace.join(&f.path);
                if !p.is_file() || should_skip(&workspace, &p) {
                    report.files_skipped += 1;
                    continue;
                }
                examine(&p, &workspace, &mut report, max);
            }
        }
    }

    report
        .findings
        .sort_by(|a, b| a.severity.cmp(&b.severity).then(a.path.cmp(&b.path)).then(a.line.cmp(&b.line)));
    Ok(report)
}

impl Report {
    pub fn render(&self) -> String {
        let mut out = format!(
            "security_scan · scope={} · {} file(s) scanned · {} skipped · {} finding(s){}\n",
            self.scope,
            self.files_read,
            self.files_skipped,
            self.findings.len(),
            if self.truncated { " (truncated)" } else { "" }
        );
        if self.findings.is_empty() {
            out.push_str(
                "\nNo rule matched. That is not proof of safety: the rule table only covers \
                 pattern-level issues (credentials, TLS off, shell/SQL injection, permissions, \
                 unpinned CI actions). Reason about the code you were asked to review.\n",
            );
            return out;
        }
        out.push_str("\nSecrets below are masked. Do not try to recover the raw value, do not print one, and do not put one in a report, commit message or file.\n\n");
        for f in &self.findings {
            out.push_str(&format!(
                "{:<5} {:<20} {}:{}  {}\n      {}\n      fix: {}\n",
                f.severity.label(),
                f.rule,
                f.path,
                f.line,
                f.what,
                f.evidence,
                f.hint
            ));
        }
        out.push_str(
            "\nEvery hit needs reading the surrounding code before you call it a bug: a rule \
             cannot tell a live credential from an example, or reachable input from a constant.\n",
        );
        crate::util::cap_chars(&out, 18_000)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn workspace(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "irixen-audit-{tag}-{}-{}",
            std::process::id(),
            crate::util::now_ms()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(dir: &Path, rel: &str, body: &str) {
        let p = dir.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(p, body).unwrap();
    }

    fn run(dir: &Path) -> Report {
        scan(dir, &ScanOptions::default()).unwrap()
    }

    #[test]
    fn a_cloud_key_is_found_and_masked() {
        let dir = workspace("cloud");
        write(&dir, "src/net.rs", "let key = \"AKIAIOSFODNN7EXAMPLE\";\n"); // irixen-audit: ignore
        let report = run(&dir);
        assert_eq!(report.findings.len(), 1, "{:?}", report.findings);
        let f = &report.findings[0];
        assert_eq!(f.rule, "cloud-credential");
        assert_eq!(f.line, 1);
        // The mask keeps a short head/tail and hides everything else.
        assert!(f.evidence.contains("***"), "{}", f.evidence);
        assert!(!f.evidence.contains("AKIAIOSFODNN7EXAMPLE"), "leaked: {}", f.evidence);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn placeholders_and_comparisons_are_not_secrets() {
        let dir = workspace("place");
        write(
            &dir,
            "src/db.py",
            "password = \"changeme\"\napi_key = \"<your-key>\"\nSECRET = os.environ[\"SECRET\"]\n\
             if password == \"hunter2hunter2\":\n    pass\n",
        );
        let report = run(&dir);
        assert!(
            report.findings.is_empty(),
            "false positives: {:?}",
            report.findings
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn env_assignment_without_quotes_is_caught() {
        let dir = workspace("env");
        write(&dir, "deploy.sh", "export GITHUB_TOKEN=ghp_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n"); // irixen-audit: ignore
        let report = run(&dir);
        assert!(report.findings.iter().any(|f| f.rule == "cloud-credential"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn key_material_is_flagged_by_name_and_skipped_when_it_is_a_sample() {
        let dir = workspace("files");
        write(&dir, "certs/server.pem", "not really a key\n");
        write(&dir, ".env.example", "TOKEN=xxxx\n");
        let report = run(&dir);
        let names: Vec<&str> = report.findings.iter().map(|f| f.path.as_str()).collect();
        assert!(names.contains(&"certs/server.pem"), "{names:?}");
        assert!(!names.contains(&".env.example"), "{names:?}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn ignored_and_dependency_directories_are_skipped() {
        let dir = workspace("skip");
        write(&dir, "node_modules/pkg/index.js", "const t = \"AKIAIOSFODNN7EXAMPLE\";\n"); // irixen-audit: ignore
        write(&dir, "src/ok.js", "const t = 1;\n");
        let report = run(&dir);
        assert!(report.findings.is_empty(), "{:?}", report.findings);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn tls_verification_and_unpinned_actions_are_reported() {
        let dir = workspace("misc");
        write(&dir, "src/http.rs", "let c = reqwest::Client::builder().danger_accept_invalid_certs(true);\n");
        write(
            &dir,
            ".github/workflows/ci.yml",
            "jobs:\n  build:\n    steps:\n      - uses: actions/checkout@main\n",
        );
        let report = run(&dir);
        assert!(report.findings.iter().any(|f| f.rule == "tls-verify-off"));
        assert!(report.findings.iter().any(|f| f.rule == "unpinned-action"));
        // Deleting the offending line makes the finding go away.
        write(&dir, ".github/workflows/ci.yml", "jobs:\n  build:\n    steps:\n      - uses: actions/checkout@v4\n");
        let after = run(&dir);
        assert!(!after.findings.iter().any(|f| f.rule == "unpinned-action"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_trailing_marker_suppresses_exactly_one_line() {
        let dir = workspace("suppress");
        // Each fixture line lives on its own source line so the marker can be
        // attached to it — this file is audited by its own rules too.
        let a = "let a = \"AKIAIOSFODNN7EXAMPLE\"; // irixen-audit: ignore\n"; // irixen-audit: ignore
        let b = "let b = \"AKIAIOSFODNN7EXAMPLE\";\n"; // irixen-audit: ignore
        let c = "let c = \"AKIAIOSFODNN7EXAMPLE\"; // irixen-audit: ignore=jwt\n"; // irixen-audit: ignore
        write(&dir, "src/a.rs", &format!("{a}{b}{c}"));
        let report = run(&dir);
        let lines: Vec<usize> = report.findings.iter().map(|f| f.line).collect();
        // Line 1 is silenced, line 2 is not, and naming another rule on line 3
        // must not silence this one — so 2 and 3 both fire.
        assert_eq!(lines, vec![2, 3], "{:?}", report.findings);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn camel_case_keys_are_caught_but_metadata_is_not() {
        let dir = workspace("camel");
        write(
            &dir,
            "src/cfg.js",
            "const secretKey = \"9f8a7b6c5d4e3f2a\";\nconst token_count = 12;\n\
             const password_hash = \"sha256:abcdef0123\";\n",
        );
        let report = run(&dir);
        assert_eq!(report.findings.len(), 1, "{:?}", report.findings);
        assert_eq!(report.findings[0].rule, "hardcoded-secret");
        assert!(report.findings[0].evidence.contains("***"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn changed_scope_reports_only_what_git_sees() {
        let dir = workspace("changed");
        let ok = std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(&dir)
            .status();
        if !ok.map(|s| s.success()).unwrap_or(false) {
            std::fs::remove_dir_all(&dir).ok();
            return; // no git on this machine: the rule is covered elsewhere
        }
        write(&dir, "tracked.rs", "fn main() {}\n");
        let _ = std::process::Command::new("git")
            .args(["add", "."])
            .current_dir(&dir)
            .status();
        write(&dir, "tracked.rs", "fn main() { let k = \"AKIAIOSFODNN7EXAMPLE\"; }\n");
        let report = scan(
            &dir,
            &ScanOptions {
                scope: Scope::Changed,
                path: None,
                max_findings: 20,
            },
        )
        .unwrap();
        assert_eq!(report.findings.len(), 1, "{:?}", report.findings);
        assert_eq!(report.findings[0].path, "tracked.rs");
        std::fs::remove_dir_all(&dir).ok();
    }
}
