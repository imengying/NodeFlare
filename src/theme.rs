use serde_json::Value;
use std::time::Duration;
use worker::{Fetch, Headers, Method, Request, Response, Result};

const STORE_URL: &str =
    "https://raw.githubusercontent.com/huilang-me/CFSM-Theme-Store/refs/heads/main/themes.json";
const COMPAT_ASSET_BASE: &str =
    "https://raw.githubusercontent.com/huilang-me/CF-Server-Monitor/main/public";
const THEME_BOOTSTRAP: &str = r#"<style>[data-cf-monitor-turnstile-verified="true"]{display:none!important;min-height:0!important}</style><script>(()=>{sessionStorage.removeItem('cf-monitor-admin-token');localStorage.removeItem('cf-monitor-admin-token');const seen=new WeakSet,holders=new Map,wrap=api=>{if(!api||typeof api.render!=='function'||seen.has(api))return api;seen.add(api);const render=api.render.bind(api),reset=typeof api.reset==='function'?api.reset.bind(api):null;api.render=(target,options={})=>{const holder=typeof target==='string'?document.querySelector(target):target,callback=options.callback,id=render(target,{...options,callback:token=>{holder?.setAttribute('data-cf-monitor-turnstile-verified','true');return callback?.(token)}});holders.set(id,holder);return id};if(reset)api.reset=id=>{if(id==null)holders.forEach(holder=>holder?.removeAttribute('data-cf-monitor-turnstile-verified'));else holders.get(id)?.removeAttribute('data-cf-monitor-turnstile-verified');return reset(id)};return api};const watch=node=>{if(node?.tagName==='SCRIPT'&&node.src.includes('challenges.cloudflare.com/turnstile/'))node.addEventListener('load',()=>wrap(window.turnstile),{once:true})},observer=new MutationObserver(records=>records.forEach(record=>record.addedNodes.forEach(watch)));observer.observe(document,{childList:true,subtree:true});document.addEventListener('load',event=>watch(event.target),true);setInterval(()=>{if(window.turnstile)wrap(window.turnstile)},250)})()</script>
"#;

#[derive(Debug)]
pub struct ThemeSource {
    pub normalized: String,
    raw_base: String,
    immutable: bool,
}

fn safe_part(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
}

pub fn parse_theme_url(value: &str) -> Option<ThemeSource> {
    let raw = value.trim().trim_end_matches('/');
    let path = raw.strip_prefix("https://github.com/")?;
    if raw.contains(['?', '#', '\\', '%']) {
        return None;
    }
    let parts = path.split('/').collect::<Vec<_>>();
    if parts.len() < 4 || parts[2] != "tree" || parts.iter().any(|part| !safe_part(part)) {
        return None;
    }
    let owner = parts[0];
    let repo = parts[1].trim_end_matches(".git");
    let reference = parts[3];
    if !safe_part(repo) {
        return None;
    }
    let suffix = if parts.len() > 4 {
        format!("/{}", parts[4..].join("/"))
    } else {
        String::new()
    };
    Some(ThemeSource {
        normalized: format!("https://github.com/{owner}/{repo}/tree/{reference}{suffix}"),
        raw_base: format!("https://raw.githubusercontent.com/{owner}/{repo}/{reference}{suffix}"),
        immutable: reference.len() == 40 && reference.chars().all(|ch| ch.is_ascii_hexdigit()),
    })
}

fn safe_asset_path(value: &str) -> bool {
    !value.is_empty()
        && !value.contains(['?', '#', '\\'])
        && value.split('/').all(|part| {
            !part.is_empty()
                && part != "."
                && part != ".."
                && part
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | '@'))
        })
}

fn content_type(path: &str) -> &'static str {
    let path = path.to_ascii_lowercase();
    if path.ends_with(".js") || path.ends_with(".mjs") {
        "application/javascript; charset=utf-8"
    } else if path.ends_with(".css") {
        "text/css; charset=utf-8"
    } else if path.ends_with(".json") || path.ends_with(".map") {
        "application/json; charset=utf-8"
    } else if path.ends_with(".svg") {
        "image/svg+xml"
    } else if path.ends_with(".png") {
        "image/png"
    } else if path.ends_with(".jpg") || path.ends_with(".jpeg") {
        "image/jpeg"
    } else if path.ends_with(".webp") {
        "image/webp"
    } else if path.ends_with(".gif") {
        "image/gif"
    } else if path.ends_with(".ico") {
        "image/x-icon"
    } else if path.ends_with(".woff2") {
        "font/woff2"
    } else if path.ends_with(".woff") {
        "font/woff"
    } else if path.ends_with(".ttf") {
        "font/ttf"
    } else {
        "application/octet-stream"
    }
}

fn rewrite_exchange_rate_urls(script: &str) -> String {
    [
        "https://api.frankfurter.dev/v1/latest?base=CNY",
        "https://api.frankfurter.app/latest?from=CNY",
        "https://open.er-api.com/v6/latest/CNY",
        "https://cdn.jsdelivr.net/npm/@fawazahmed0/currency-api@latest/v1/currencies/cny.json",
        "https://latest.currency-api.pages.dev/v1/currencies/cny.json",
    ]
    .into_iter()
    .fold(script.to_string(), |script, url| {
        script.replace(url, "/api/exchange-rates")
    })
}

async fn request(url: &str, timeout: Duration) -> Result<worker::Response> {
    let request = Request::new(url, Method::Get)?;
    request.headers().set("Accept", "*/*")?;
    request
        .headers()
        .set("User-Agent", "CF-Monitor-Theme-Proxy")?;
    let controller = worker::AbortController::default();
    let signal = controller.signal();
    worker::wasm_bindgen_futures::spawn_local(async move {
        worker::Delay::from(timeout).await;
        controller.abort();
    });
    Fetch::Request(request).send_with_signal(&signal).await
}

fn fallback_store() -> Value {
    serde_json::json!({
        "schema": 1,
        "themes": [
            {
                "id": "emerald",
                "title": "Emerald",
                "cover": "https://raw.githubusercontent.com/Tokinx/cf-server-monitor-theme-emerald/main/docs/preview.png",
                "tags": ["Emerald", "Earth"],
                "description": { "zh-CN": "翡翠配色的 CF Server Monitor 主题。", "en": "An emerald theme for CF Server Monitor." },
                "url": "https://github.com/Tokinx/cf-server-monitor-theme-emerald",
                "branch": "build",
                "author": "Tokinx"
            },
            {
                "id": "pulse",
                "title": "Pulse",
                "cover": "https://raw.githubusercontent.com/loongkong/cf-server-monitor-theme-pulse/main/docs/preview.svg",
                "tags": ["Pulse", "Minimal", "Mono"],
                "description": { "zh-CN": "极简风格的 CF Server Monitor 主题。", "en": "A minimal CF Server Monitor theme." },
                "url": "https://github.com/loongkong/cf-server-monitor-theme-pulse",
                "branch": "main",
                "author": "loongkong"
            }
        ]
    })
}

async fn mutable_response(mut response: Response) -> Result<Response> {
    let status = response.status_code();
    let headers = Headers::new();
    for (name, value) in response.headers().entries() {
        headers.append(&name, &value)?;
    }
    let body = response.bytes().await?;
    Ok(Response::from_bytes(body)?
        .with_status(status)
        .with_headers(headers))
}

async fn cached_request(url: &str, ttl: u32, timeout: Duration) -> Result<Response> {
    let response = request(url, timeout).await?;
    let mut response = mutable_response(response).await?;
    if (200..300).contains(&response.status_code()) {
        response.headers_mut().set(
            "Cache-Control",
            &format!("public, max-age={ttl}, s-maxage={ttl}"),
        )?;
    }
    Ok(response)
}

pub async fn store() -> Result<Value> {
    let mut response = match cached_request(STORE_URL, 300, Duration::from_secs(8)).await {
        Ok(response) => response,
        Err(_) => return Ok(fallback_store()),
    };
    if !(200..300).contains(&response.status_code()) {
        return Ok(fallback_store());
    }
    response.json().await.or_else(|_| Ok(fallback_store()))
}

fn valid_setting_key(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-'))
}

fn short_text(value: Option<&Value>, limit: usize) -> Option<String> {
    value
        .and_then(Value::as_str)
        .filter(|value| value.chars().count() <= limit)
        .map(str::to_string)
}

fn sanitize_settings_schema(value: Value) -> Value {
    let Some(settings) = value.get("settings").and_then(Value::as_array) else {
        return serde_json::json!({ "schema": 1, "source": "third-party", "settings": [] });
    };
    let mut fields = Vec::new();
    for field in settings.iter().take(40) {
        let Some(object) = field.as_object() else {
            continue;
        };
        let Some(key) = object
            .get("key")
            .and_then(Value::as_str)
            .filter(|key| valid_setting_key(key))
        else {
            continue;
        };
        let Some(label) = short_text(object.get("label"), 80) else {
            continue;
        };
        let Some(kind) = object.get("type").and_then(Value::as_str).filter(|kind| {
            matches!(
                *kind,
                "text" | "textarea" | "url" | "color" | "select" | "toggle" | "number"
            )
        }) else {
            continue;
        };
        let mut clean = serde_json::Map::new();
        clean.insert("key".into(), Value::String(key.to_string()));
        clean.insert("label".into(), Value::String(label));
        clean.insert("type".into(), Value::String(kind.to_string()));
        if let Some(placeholder) = short_text(object.get("placeholder"), 160) {
            clean.insert("placeholder".into(), Value::String(placeholder));
        }
        if let Some(default) = object.get("default").filter(|value| {
            value.is_boolean()
                || value.is_number()
                || value
                    .as_str()
                    .is_some_and(|text| text.chars().count() <= 500)
        }) {
            clean.insert("default".into(), default.clone());
        }
        for key in ["min", "max", "step"] {
            if let Some(number) = object
                .get(key)
                .and_then(Value::as_f64)
                .filter(|number| number.is_finite())
            {
                clean.insert(key.into(), serde_json::json!(number));
            }
        }
        if kind == "select" {
            let options = object
                .get("options")
                .and_then(Value::as_array)
                .map(|options| {
                    options
                        .iter()
                        .take(40)
                        .filter_map(|option| {
                            let label = short_text(option.get("label"), 80)?;
                            let value = short_text(option.get("value"), 160)?;
                            Some(serde_json::json!({ "label": label, "value": value }))
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            if options.is_empty() {
                continue;
            }
            clean.insert("options".into(), Value::Array(options));
        }
        fields.push(Value::Object(clean));
    }
    serde_json::json!({ "schema": 1, "source": "third-party", "settings": fields })
}

fn builtin_settings_schema() -> Value {
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

pub async fn settings_schema(theme_url: &str) -> Result<Value> {
    if theme_url.trim().is_empty() {
        return Ok(builtin_settings_schema());
    }
    let Some(source) = parse_theme_url(theme_url) else {
        return Ok(serde_json::json!({ "schema": 1, "source": "third-party", "settings": [] }));
    };
    let mut response = match cached_request(
        &format!("{}/theme-settings.json", source.raw_base),
        300,
        Duration::from_secs(8),
    )
    .await
    {
        Ok(response) => response,
        Err(_) => {
            return Ok(serde_json::json!({ "schema": 1, "source": "third-party", "settings": [] }))
        }
    };
    if !(200..300).contains(&response.status_code()) {
        return Ok(serde_json::json!({ "schema": 1, "source": "third-party", "settings": [] }));
    }
    let value = response
        .json()
        .await
        .unwrap_or_else(|_| serde_json::json!({}));
    Ok(sanitize_settings_schema(value))
}

pub async fn load_index(theme_url: &str, title: &str) -> Result<Option<String>> {
    let Some(source) = parse_theme_url(theme_url) else {
        return Ok(None);
    };
    let mut response = match cached_request(
        &format!("{}/index.html", source.raw_base),
        300,
        Duration::from_secs(20),
    )
    .await
    {
        Ok(response) => response,
        Err(_) => return Ok(None),
    };
    if !(200..300).contains(&response.status_code()) {
        return Ok(None);
    }
    let mut html = response.text().await?;
    let lower = html.to_ascii_lowercase();
    if lower.contains("/src/main.ts")
        || lower.contains("/src/main.tsx")
        || lower.contains("src=\"src/")
        || lower.contains("src='./src/")
    {
        return Ok(None);
    }
    for (from, to) in [
        ("src=\"./assets/", "src=\"/assets/"),
        ("href=\"./assets/", "href=\"/assets/"),
        ("src='\x2e/assets/", "src='/assets/"),
        ("href='\x2e/assets/", "href='/assets/"),
        ("src=\"assets/", "src=\"/assets/"),
        ("href=\"assets/", "href=\"/assets/"),
    ] {
        html = html.replace(from, to);
    }
    let safe_title = title
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;");
    if let Some(index) = html.to_ascii_lowercase().find("</head>") {
        html.insert_str(index, &format!("<title>{safe_title}</title>\n"));
    }
    html.insert_str(0, THEME_BOOTSTRAP);
    Ok(Some(html))
}

pub async fn asset(theme_url: &str, path: &str) -> Result<Response> {
    let Some(source) = parse_theme_url(theme_url) else {
        return Response::error("Invalid theme", 400);
    };
    if !safe_asset_path(path) {
        return Response::error("Not Found", 404);
    }
    let ttl = if source.immutable { 31_536_000 } else { 3600 };
    let mut remote = cached_request(
        &format!("{}/assets/{path}", source.raw_base),
        ttl,
        Duration::from_secs(20),
    )
    .await?;
    let status = remote.status_code();
    if !(200..300).contains(&status) {
        return Response::error("Theme asset not found", status);
    }
    let bytes = remote.bytes().await?;
    let bytes = if path.ends_with(".js") || path.ends_with(".mjs") {
        match String::from_utf8(bytes) {
            Ok(script) => rewrite_exchange_rate_urls(&script).into_bytes(),
            Err(error) => error.into_bytes(),
        }
    } else {
        bytes
    };
    let mut response = Response::from_bytes(bytes)?.with_status(status);
    response
        .headers_mut()
        .set("Content-Type", content_type(path))?;
    response.headers_mut().set(
        "Cache-Control",
        if source.immutable {
            "public, max-age=31536000, immutable"
        } else {
            "public, max-age=3600"
        },
    )?;
    response
        .headers_mut()
        .set("X-Content-Type-Options", "nosniff")?;
    Ok(response)
}

pub async fn compatibility_asset(path: &str) -> Result<Response> {
    let allowed = path == "favicon.ico"
        || path.strip_prefix("flags/").is_some_and(safe_asset_path)
        || path.strip_prefix("os-icons/").is_some_and(safe_asset_path);
    if !allowed {
        return Response::error("Not Found", 404);
    }
    let mut remote = cached_request(
        &format!("{COMPAT_ASSET_BASE}/{path}"),
        86_400,
        Duration::from_secs(20),
    )
    .await?;
    let status = remote.status_code();
    if !(200..300).contains(&status) {
        return Response::error("Theme asset not found", status);
    }
    let bytes = remote.bytes().await?;
    let mut response = Response::from_bytes(bytes)?.with_status(status);
    response
        .headers_mut()
        .set("Content-Type", content_type(path))?;
    response
        .headers_mut()
        .set("Cache-Control", "public, max-age=86400")?;
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::{
        builtin_settings_schema, parse_theme_url, rewrite_exchange_rate_urls, safe_asset_path,
        sanitize_settings_schema,
    };

    #[test]
    fn accepts_only_github_tree_themes() {
        let valid =
            parse_theme_url("https://github.com/Tokinx/cf-server-monitor-theme-emerald/tree/build")
                .expect("valid theme");
        assert_eq!(
            valid.normalized,
            "https://github.com/Tokinx/cf-server-monitor-theme-emerald/tree/build"
        );
        assert!(parse_theme_url("https://example.com/theme/tree/main").is_none());
        assert!(parse_theme_url("https://github.com/a/b/blob/main").is_none());
        assert!(parse_theme_url("https://github.com/a/b/tree/main/../private").is_none());
    }

    #[test]
    fn rejects_theme_asset_traversal() {
        assert!(safe_asset_path("chunks/index-Ab_1.js"));
        assert!(safe_asset_path("@scope/theme.js"));
        assert!(!safe_asset_path("../secret"));
        assert!(!safe_asset_path("assets/./index.js"));
        assert!(!safe_asset_path("assets//index.js"));
        assert!(!safe_asset_path("a\\b.js"));
    }

    #[test]
    fn routes_theme_exchange_rates_through_the_worker() {
        let script = "fetch('https://api.frankfurter.dev/v1/latest?base=CNY');\
                      fetch('https://cdn.jsdelivr.net/npm/@fawazahmed0/currency-api@latest/v1/currencies/cny.json')";
        let rewritten = rewrite_exchange_rate_urls(script);
        assert_eq!(rewritten.matches("/api/exchange-rates").count(), 2);
        assert!(!rewritten.contains("frankfurter.dev"));
        assert!(!rewritten.contains("jsdelivr.net"));
    }

    #[test]
    fn sanitizes_frontend_theme_settings() {
        let schema = sanitize_settings_schema(serde_json::json!({
            "settings": [
                { "key": "accent", "label": "强调色", "type": "color", "default": "#0f766e" },
                { "key": "layout", "label": "布局", "type": "select", "options": [{ "label": "紧凑", "value": "compact" }] },
                { "key": "bad key", "label": "Bad", "type": "text" }
            ]
        }));
        assert_eq!(schema["settings"].as_array().unwrap().len(), 2);
        assert_eq!(schema["settings"][0]["key"], "accent");
    }

    #[test]
    fn exposes_builtin_glass_theme_settings() {
        let schema = builtin_settings_schema();
        let settings = schema["settings"].as_array().expect("settings");
        assert!(settings.iter().any(|field| field["key"] == "assetCurrency"));
        assert!(settings.iter().any(|field| field["key"] == "enableBlur"));
        assert!(settings.iter().any(|field| field["key"] == "showOnline"));
    }
}
