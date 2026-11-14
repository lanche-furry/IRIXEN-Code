//! HTTP plumbing shared by every provider.
//!
//! Three things used to be silently wrong here:
//!   1. `reqwest` sends no `User-Agent` by default, and several vendor gateways
//!      (Cloudflare-fronted ones especially) answer 403 to that.
//!   2. Only `connect_timeout` was set, so a stalled response hung forever and
//!      looked like "cannot reach the provider".
//!   3. Proxy handling was whatever the environment happened to contain. A GUI
//!      app launched from Explorer inherits nothing, so the machine's system
//!      proxy was ignored and every request went out direct.
//!
//! Everything below exists to make a failure say *why* it failed.

use std::sync::OnceLock;
use std::time::Duration;

pub const USER_AGENT: &str = concat!(
    "IRIXEN-Code/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/lanche-furry/IRIXEN-Code)"
);

/// Endpoint paths people paste into "Base URL" by mistake.
const ENDPOINT_SUFFIXES: &[&str] = &[
    "/chat/completions",
    "/v1/chat/completions",
    "/completions",
    "/messages",
    "/streamgeneratecontent",
    "/generatecontent",
    "/models",
];

/// Turn whatever the user typed into a clean origin+path that endpoints can be
/// appended to.
///
/// `api.deepseek.com` → `https://api.deepseek.com`
/// `https://api.openai.com/v1/` → `https://api.openai.com/v1`
/// `https://x.dev/v1/chat/completions` → `https://x.dev/v1`
pub fn normalize_base(raw: &str) -> String {
    let mut s = raw.trim().to_string();
    if s.is_empty() {
        return s;
    }

    // No scheme: assume https, except for loopback where http is the norm.
    if !s.contains("://") {
        let loopback = s.starts_with("localhost")
            || s.starts_with("127.")
            || s.starts_with("192.168.")
            || s.starts_with("0.0.0.0")
            || s.starts_with('[');
        s = if loopback {
            format!("http://{s}")
        } else {
            format!("https://{s}")
        };
    }

    while s.ends_with('/') {
        s.pop();
    }

    // Strip a full endpoint the user pasted instead of the root.
    loop {
        let lower = s.to_lowercase();
        match ENDPOINT_SUFFIXES.iter().find(|sfx| lower.ends_with(**sfx)) {
            Some(sfx) => {
                s.truncate(s.len() - sfx.len());
                while s.ends_with('/') {
                    s.pop();
                }
            }
            None => break,
        }
    }
    s
}

/// `https://api.x.dev/v1` + `/models` → `https://api.x.dev/v1/models`,
/// without ever doubling a slash.
pub fn join(base: &str, path: &str) -> String {
    let b = base.trim_end_matches('/');
    if path.is_empty() {
        return b.to_string();
    }
    if path.starts_with('/') {
        format!("{b}{path}")
    } else {
        format!("{b}/{path}")
    }
}

// ---------------------------------------------------------------------------
// proxy resolution
// ---------------------------------------------------------------------------

fn env_proxy() -> Option<String> {
    // Order matters: HTTPS first, then the generic ones.
    const KEYS: &[&str] = &[
        "HTTPS_PROXY",
        "https_proxy",
        "ALL_PROXY",
        "all_proxy",
        "HTTP_PROXY",
        "http_proxy",
    ];
    for k in KEYS {
        if let Ok(v) = std::env::var(k) {
            let v = v.trim().to_string();
            if !v.is_empty() {
                return Some(with_scheme(&v));
            }
        }
    }
    None
}

fn with_scheme(v: &str) -> String {
    if v.contains("://") {
        v.to_string()
    } else if v.starts_with("socks") {
        format!("socks5://{v}")
    } else {
        format!("http://{v}")
    }
}

/// Read the WinINET proxy (`Settings > Network > Proxy`) — the one every other
/// Windows app honours. Registry is queried through `reg.exe` so we pull in no
/// extra crate; the result is cached for the process lifetime.
#[cfg(windows)]
fn system_proxy() -> Option<String> {
    use std::os::windows::process::CommandExt;
    use std::process::Command;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings";

    fn query(key: &str, name: &str) -> Option<String> {
        let out = Command::new("reg")
            .args(["query", key, "/v", name])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&out.stdout);
        // `    ProxyServer    REG_SZ    127.0.0.1:7900`
        text.lines()
            .find(|l| l.contains("REG_SZ"))
            .and_then(|l| l.split("REG_SZ").nth(1))
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    }

    let enabled = query(KEY, "ProxyEnable")
        .map(|v| v.trim() != "0")
        .unwrap_or(false);
    if !enabled {
        return None;
    }
    let server = query(KEY, "ProxyServer")?;

    // `http=host:port;https=host:port` or a bare `host:port`.
    let pick = if server.contains('=') {
        let mut https = None;
        let mut http = None;
        for part in server.split(';') {
            let mut it = part.splitn(2, '=');
            let (k, v) = (it.next().unwrap_or("").trim(), it.next().unwrap_or("").trim());
            if v.is_empty() {
                continue;
            }
            if k.eq_ignore_ascii_case("https") {
                https = Some(v.to_string());
            } else if k.eq_ignore_ascii_case("http") {
                http = Some(v.to_string());
            }
        }
        https.or(http)?
    } else {
        server
    };
    // If only the host is given, WinINET implies http://.
    (!pick.is_empty()).then(|| with_scheme(&pick))
}

#[cfg(not(windows))]
fn system_proxy() -> Option<String> {
    None
}

/// Resolution order: explicit setting → environment → OS proxy → direct.
pub fn resolve_proxy(explicit: &str) -> Option<String> {
    let e = explicit.trim();
    if e.eq_ignore_ascii_case("none") || e.eq_ignore_ascii_case("direct") {
        return None;
    }
    if !e.is_empty() {
        return Some(with_scheme(e));
    }
    static CACHE: OnceLock<Option<String>> = OnceLock::new();
    CACHE
        .get_or_init(|| env_proxy().or_else(system_proxy))
        .clone()
}

pub fn proxy_label(explicit: &str) -> String {
    match resolve_proxy(explicit) {
        Some(p) => p,
        None => "direct".to_string(),
    }
}

// ---------------------------------------------------------------------------
// client
// ---------------------------------------------------------------------------

pub fn build_client(explicit_proxy: &str) -> reqwest::Client {
    let mut b = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(Duration::from_secs(20))
        // Per-read, not whole-request: SSE streams legitimately stay open for
        // minutes, but a silent socket for two minutes means it is dead.
        .read_timeout(Duration::from_secs(150))
        .pool_idle_timeout(Duration::from_secs(60));

    b = match resolve_proxy(explicit_proxy) {
        Some(p) => match reqwest::Proxy::all(&p) {
            Ok(px) => {
                // A system/explicit proxy must never swallow the local
                // Web2API endpoint: `http://127.0.0.1:8791/v1/models` sent to
                // an upstream proxy comes back as 502 / connection refused and
                // the whole bridge looks broken.
                let bypass = reqwest::NoProxy::from_string("localhost,127.0.0.1,0.0.0.0,::1");
                b.proxy(px.no_proxy(bypass))
            }
            Err(_) => b.no_proxy(),
        },
        // Already resolved; don't let reqwest pick up a different env value.
        None => b.no_proxy(),
    };

    b.build().unwrap_or_else(|_| reqwest::Client::new())
}

// ---------------------------------------------------------------------------
// error text
// ---------------------------------------------------------------------------

/// Turn a transport error into something the user can act on.
pub fn explain_transport(e: &reqwest::Error, url: &str, proxy: &str) -> String {
    let low = e.to_string().to_lowercase();
    let via = if proxy == "direct" {
        "直连".to_string()
    } else {
        format!("经代理 {proxy}")
    };

    let hint = if e.is_timeout() {
        "请求超时。检查网络，或在设置 → 网络里填写代理地址。"
    } else if low.contains("certificate") || low.contains("tls") || low.contains("ssl") {
        "TLS 握手失败。如果是企业代理或抓包工具，请把它的根证书装进系统；否则关闭代理再试。"
    } else if low.contains("dns") || low.contains("failed to lookup") {
        "域名解析失败。确认 Base URL 拼写正确，或检查 DNS / 代理是否可用。"
    } else if low.contains("connection refused") || low.contains("actively refused") {
        "连接被拒绝。代理端口可能没启动，或 Base URL 的端口不对。"
    } else if low.contains("builder") || low.contains("relative url") || low.contains("invalid url") {
        "URL 格式不对。需要形如 https://api.openai.com/v1 的完整地址。"
    } else if low.contains("proxy") {
        "代理连接失败。检查代理地址与端口，或把它改成 none 直连。"
    } else if low.contains("unexpected eof") || low.contains("connection reset") {
        "连接被中断。可能是代理不稳定，或该域名需要走代理。"
    } else {
        "网络请求失败。先确认浏览器能打开同一个地址，再检查代理设置。"
    };

    format!("网络错误（{via}）：{hint}\n  地址：{url}\n  原始信息：{e}")
}

/// Turn a non-2xx response into an explanation. The status alone rarely tells
/// you which of the three usual mistakes happened.
pub fn explain_status(status: u16, body: &str, url: &str) -> String {
    let snippet: String = body.chars().take(400).collect();
    let hint = match status {
        401 | 403 => "鉴权失败。检查 API Key；部分服务商还要求额外的 header（如 HTTP-Referer）。若 Key 正确，可能是该地区被限制，需要代理。",
        404 => "路径不存在。Base URL 多写或少写了 /v1，或者协议选错了（OpenAI / Anthropic / Gemini）。",
        429 => "触发限流或余额不足。检查额度与并发。",
        400 => "请求被拒。通常是模型名不存在，或该模型不支持当前的思考强度参数。",
        502 if snippet.contains("官网")
            || snippet.contains("登录")
            || snippet.contains("风控")
            || snippet.contains("验证页") =>
        {
            "Web2API 官网会话不可用。按下面「响应」里的提示打开官网登录 / 完成人机验证后重试；这不是 API Key 或模型名的问题。"
        }
        500..=599 => "服务端错误。稍后重试；若持续出现，换一个模型试试。",
        _ => "请求未被接受。检查 Base URL、协议和 Key 是否匹配。",
    };
    format!("接口返回 {status}：{hint}\n  地址：{url}\n  响应：{snippet}")
}
