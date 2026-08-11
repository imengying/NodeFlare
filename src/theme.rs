use futures_util::TryStreamExt;
use serde_json::Value;
use worker::{Error, Fetch, Method, Request, Response, Result, Url};

pub const BUILTIN_THEME_ID: &str = "builtin-komari-glass";
pub const BUILTIN_THEME_NAME: &str = "Komari Glass";
pub const INDEX_MAX_BYTES: usize = 4 * 1024 * 1024;
pub const ASSET_MAX_BYTES: usize = 16 * 1024 * 1024;

pub struct ResolvedTheme {
    pub source_url: String,
    pub resolved_url: String,
}

pub fn normalize_url(value: &str) -> Option<String> {
    let raw = value.trim();
    if !(12..=2048).contains(&raw.len()) || raw.contains('\\') {
        return None;
    }
    let parsed = Url::parse(raw).ok()?;
    if parsed.scheme() != "https"
        || parsed.host_str() != Some("github.com")
        || parsed.port().is_some()
        || parsed.username() != ""
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return None;
    }

    let path = parsed.path().trim_matches('/');
    let parts: Vec<_> = path.split('/').filter(|part| !part.is_empty()).collect();
    if parts.len() < 4
        || parts[2] != "tree"
        || parts.iter().any(|part| {
            part.is_empty()
                || !part
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric() || ".-_".contains(character))
                || matches!(*part, "." | "..")
        })
    {
        return None;
    }
    Some(format!("https://github.com/{}", parts.join("/")))
}

pub fn resolve_url(value: &str) -> Result<ResolvedTheme> {
    let source_url =
        normalize_url(value).ok_or_else(|| Error::RustError("主题 URL 格式无效".to_string()))?;
    let parsed = Url::parse(&source_url)?;

    let parts: Vec<_> = parsed
        .path()
        .trim_matches('/')
        .split('/')
        .filter(|part| !part.is_empty())
        .collect();
    let owner = parts[0];
    let repository = parts[1];
    let reference = parts[3];
    let subdirectory = parts[4..].join("/");
    let suffix = if subdirectory.is_empty() {
        String::new()
    } else {
        format!("/{subdirectory}")
    };
    Ok(ResolvedTheme {
        source_url,
        resolved_url: format!(
            "https://raw.githubusercontent.com/{owner}/{repository}/{reference}{suffix}"
        ),
    })
}

fn remote_url(base: &str, relative: &str) -> String {
    format!("{}/{}", base.trim_end_matches('/'), relative)
}

fn raw_github_base(base: &str) -> Option<Url> {
    let parsed = Url::parse(&format!("{}/", base.trim_end_matches('/'))).ok()?;
    if parsed.scheme() != "https"
        || parsed.host_str() != Some("raw.githubusercontent.com")
        || parsed.port().is_some()
        || parsed.username() != ""
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return None;
    }
    let parts = parsed
        .path()
        .trim_matches('/')
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    (parts.len() >= 3
        && parts.iter().all(|part| {
            !matches!(*part, "." | "..")
                && part
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric() || ".-_".contains(character))
        }))
    .then_some(parsed)
}

pub fn asset_url(base: &str, path: &str) -> Option<String> {
    let relative = path.strip_prefix("/assets/")?;
    if relative.is_empty()
        || relative.len() > 1024
        || relative
            .chars()
            .any(|character| character.is_control() || matches!(character, '%' | '\\' | '?' | '#'))
        || relative
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return None;
    }

    let base = raw_github_base(base)?;
    let remote = base.join(&format!("assets/{relative}")).ok()?;
    let expected_path = format!("{}assets/{relative}", base.path());
    if remote.origin() != base.origin() || remote.path() != expected_path {
        return None;
    }
    Some(remote.to_string())
}

pub fn index_url(base: &str) -> Option<String> {
    raw_github_base(base).map(|base| remote_url(base.as_str(), "index.html"))
}

pub async fn read_response_limited(
    response: &mut Response,
    limit: usize,
) -> Result<Option<Vec<u8>>> {
    let declared = response
        .headers()
        .get("Content-Length")?
        .and_then(|value| value.parse::<usize>().ok());
    if declared.is_some_and(|length| length == 0 || length > limit) {
        return Ok(None);
    }

    let mut body = Vec::with_capacity(declared.unwrap_or(0).min(limit));
    let mut stream = response.stream()?;
    while let Some(mut chunk) = stream.try_next().await? {
        let Some(length) = body.len().checked_add(chunk.len()) else {
            return Ok(None);
        };
        if length > limit {
            return Ok(None);
        }
        body.append(&mut chunk);
    }
    Ok((!body.is_empty()).then_some(body))
}

pub async fn validate_remote(base: &str) -> Result<()> {
    let index = index_url(base)
        .ok_or_else(|| Error::RustError("主题资源地址不是 GitHub Raw 地址".to_string()))?;
    let request = Request::new(&index, Method::Get)?;
    let mut response = Fetch::Request(request).send().await?;
    let status = response.status_code();
    if !(200..300).contains(&status) {
        return Err(Error::RustError(format!(
            "主题 index.html 返回 HTTP {status}"
        )));
    }
    let body = read_response_limited(&mut response, INDEX_MAX_BYTES)
        .await?
        .ok_or_else(|| Error::RustError("主题 index.html 内容无效".to_string()))?;
    let body = String::from_utf8(body)
        .map_err(|_| Error::RustError("主题 index.html 不是 UTF-8 文本".to_string()))?;
    if body.trim().is_empty() {
        return Err(Error::RustError("主题 index.html 内容无效".to_string()));
    }
    Ok(())
}

pub fn settings_schema() -> Value {
    let currencies = [
        "CNY", "USD", "HKD", "EUR", "GBP", "JPY", "RUB", "CHF", "INR", "VND", "THB", "CAD",
    ];
    serde_json::json!({
        "schema": 1,
        "source": "builtin",
        "settings": [
            {
                "key": "assetCurrency",
                "label": "资产折算币种",
                "type": "select",
                "default": "CNY",
                "options": currencies.iter().map(|currency| {
                    serde_json::json!({ "label": currency, "value": currency })
                }).collect::<Vec<_>>()
            },
            {
                "key": "enableBlur",
                "label": "启用毛玻璃效果",
                "type": "toggle",
                "default": true
            },
            {
                "key": "showOnline",
                "label": "总览显示在线节点",
                "type": "toggle",
                "default": true
            }
        ]
    })
}

#[cfg(test)]
mod tests {
    use super::{asset_url, normalize_url, resolve_url, settings_schema};

    #[test]
    fn exposes_builtin_glass_theme_settings() {
        let schema = settings_schema();
        let settings = schema["settings"].as_array().expect("settings");
        assert!(settings.iter().any(|field| field["key"] == "assetCurrency"));
        assert!(settings.iter().any(|field| field["key"] == "enableBlur"));
        assert_eq!(schema["source"], "builtin");
    }

    #[test]
    fn accepts_only_github_tree_urls() {
        assert_eq!(
            normalize_url("https://github.com/acme/theme/tree/abc123").as_deref(),
            Some("https://github.com/acme/theme/tree/abc123")
        );
        assert!(normalize_url("https://themes.example.com/nodeflare").is_none());
        assert!(normalize_url("http://themes.example.com/theme").is_none());
        assert!(normalize_url("https://user@example.com/theme").is_none());
        assert!(normalize_url("https://github.com/acme/theme").is_none());
        assert_eq!(
            resolve_url("https://github.com/acme/theme/tree/main/dist")
                .expect("GitHub theme URL")
                .resolved_url,
            "https://raw.githubusercontent.com/acme/theme/main/dist"
        );
    }

    #[test]
    fn limits_proxy_paths_to_theme_assets() {
        assert_eq!(
            asset_url(
                "https://raw.githubusercontent.com/acme/theme/main",
                "/assets/app.js"
            )
            .as_deref(),
            Some("https://raw.githubusercontent.com/acme/theme/main/assets/app.js")
        );
        assert!(asset_url("https://example.com/theme", "/assets/app.js").is_none());
        assert!(asset_url("https://example.com/theme", "/logo.svg").is_none());
        assert!(asset_url("https://example.com/theme", "/assets/../secret").is_none());
        assert!(asset_url("https://example.com/theme", "/assets/%2e%2e/secret").is_none());
        assert!(asset_url("https://example.com/theme", "/assets/..\\secret").is_none());
        assert!(asset_url("https://example.com/theme", "/assets/a%2fb.js").is_none());
    }
}
