use serde::{Deserialize, Serialize};
use worker::{wasm_bindgen::JsValue, Fetch, Method, Request, RequestInit, Result};

const SITEVERIFY_URL: &str = "https://challenges.cloudflare.com/turnstile/v0/siteverify";

#[derive(Serialize)]
struct VerifyRequest<'a> {
    secret: &'a str,
    response: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    remoteip: Option<&'a str>,
}

#[derive(Deserialize)]
struct VerifyResponse {
    success: bool,
}

pub async fn verify(token: &str, secret: &str, remote_ip: Option<&str>) -> Result<bool> {
    if token.is_empty() || token.len() > 2048 || secret.is_empty() {
        return Ok(false);
    }

    let body = serde_json::to_string(&VerifyRequest {
        secret,
        response: token,
        remoteip: remote_ip,
    })?;
    let mut init = RequestInit::new();
    init.with_method(Method::Post)
        .with_body(Some(JsValue::from_str(&body)));
    let request = Request::new_with_init(SITEVERIFY_URL, &init)?;
    request.headers().set("Content-Type", "application/json")?;

    let mut response = Fetch::Request(request).send().await?;
    if !(200..300).contains(&response.status_code()) {
        return Ok(false);
    }
    let result: VerifyResponse = response.json().await?;
    Ok(result.success)
}
