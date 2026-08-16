use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use hmac::{Hmac, KeyInit, Mac};
use pbkdf2::pbkdf2_hmac;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use worker::{
    js_sys::{Function, Reflect, Uint8Array},
    wasm_bindgen::{JsCast, JsValue},
    Date, Env, Request,
};

pub const ADMIN_SESSION_SECONDS: i64 = 7 * 24 * 60 * 60;
const TURNSTILE_PROOF_SECONDS: i64 = 60 * 60;
const THEME_PREVIEW_SECONDS: i64 = 10 * 60;
const SERVER_PASSWORD_ROUNDS: u32 = 10_000;
const JWT_HEADER: &str = r#"{"alg":"HS256","typ":"JWT"}"#;

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Deserialize, Serialize)]
struct AdminClaims {
    sub: String,
    iat: i64,
    exp: i64,
}

pub fn sha256_hex(value: &str) -> String {
    let digest = Sha256::digest(value.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn secure_eq(left: &str, right: &str) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.as_bytes()
        .iter()
        .zip(right.as_bytes())
        .fold(0_u8, |diff, (a, b)| diff | (a ^ b))
        == 0
}

fn admin_secret(env: &Env) -> Option<String> {
    env.secret("ADMIN_PASSWORD").ok().map(|v| v.to_string())
}

fn password_digest(password_derived: &str, salt: &str, rounds: u32) -> String {
    let mut digest = [0_u8; 32];
    pbkdf2_hmac::<Sha256>(
        password_derived.as_bytes(),
        salt.as_bytes(),
        rounds,
        &mut digest,
    );
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub fn hash_password(password_derived: &str, salt: &str) -> String {
    format!(
        "pbkdf2_sha256_client_v1${SERVER_PASSWORD_ROUNDS}${salt}${}",
        password_digest(password_derived, salt, SERVER_PASSWORD_ROUNDS)
    )
}

pub fn random_salt() -> Option<String> {
    let global = worker::js_sys::global();
    let crypto = Reflect::get(&global, &JsValue::from_str("crypto")).ok()?;
    let get_random_values = Reflect::get(&crypto, &JsValue::from_str("getRandomValues"))
        .ok()?
        .dyn_into::<Function>()
        .ok()?;
    let salt = Uint8Array::new_with_length(16);
    get_random_values.call1(&crypto, &salt).ok()?;
    Some(
        salt.to_vec()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
    )
}

fn verify_password_hash(password_derived: &str, encoded: &str) -> bool {
    let mut parts = encoded.split('$');
    let Some(algorithm) = parts.next() else {
        return false;
    };
    let Some(rounds) = parts.next().and_then(|value| value.parse::<u32>().ok()) else {
        return false;
    };
    let Some(salt) = parts.next() else {
        return false;
    };
    let Some(expected) = parts.next() else {
        return false;
    };
    if algorithm != "pbkdf2_sha256_client_v1"
        || rounds != SERVER_PASSWORD_ROUNDS
        || salt.len() < 32
        || parts.next().is_some()
    {
        return false;
    }
    secure_eq(&password_digest(password_derived, salt, rounds), expected)
}

pub fn verify_credentials(
    env: &Env,
    configured_username: &str,
    password_hash: &str,
    username: &str,
    password: &str,
    password_derived: &str,
) -> bool {
    let expected_username = configured_username.trim();
    let username_valid =
        !expected_username.is_empty() && secure_eq(expected_username, username.trim());
    let password_valid = if password_hash.is_empty() {
        admin_secret(env)
            .map(|expected| secure_eq(&expected, password))
            .unwrap_or(false)
    } else {
        verify_password_hash(password_derived, password_hash)
    };
    username_valid && password_valid
}

fn signing_secret(env: &Env, password_hash: &str) -> Option<String> {
    if !password_hash.is_empty() {
        return Some(password_hash.to_string());
    }
    admin_secret(env)
}

fn create_admin_jwt_with_secret(secret: &str, issued_at: i64) -> Option<String> {
    let header = URL_SAFE_NO_PAD.encode(JWT_HEADER);
    let claims = URL_SAFE_NO_PAD.encode(
        serde_json::to_vec(&AdminClaims {
            sub: "admin".to_string(),
            iat: issued_at,
            exp: issued_at + ADMIN_SESSION_SECONDS,
        })
        .ok()?,
    );
    let unsigned = format!("{header}.{claims}");
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).ok()?;
    mac.update(unsigned.as_bytes());
    let signature = URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());
    Some(format!("{unsigned}.{signature}"))
}

fn verify_admin_jwt(token: &str, secret: &str, now: i64) -> bool {
    let mut parts = token.split('.');
    let (Some(header), Some(claims), Some(signature), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return false;
    };
    if header != URL_SAFE_NO_PAD.encode(JWT_HEADER) {
        return false;
    }
    let Ok(signature) = URL_SAFE_NO_PAD.decode(signature) else {
        return false;
    };
    let Ok(mut mac) = HmacSha256::new_from_slice(secret.as_bytes()) else {
        return false;
    };
    mac.update(format!("{header}.{claims}").as_bytes());
    if mac.verify_slice(&signature).is_err() {
        return false;
    }
    let Ok(payload) = URL_SAFE_NO_PAD.decode(claims) else {
        return false;
    };
    let Ok(claims) = serde_json::from_slice::<AdminClaims>(&payload) else {
        return false;
    };
    claims.sub == "admin"
        && claims.iat <= now + 60
        && claims.exp >= now
        && claims.exp == claims.iat + ADMIN_SESSION_SECONDS
}

pub fn create_admin_jwt(env: &Env, password_hash: &str) -> Option<String> {
    let secret = signing_secret(env, password_hash)?;
    create_admin_jwt_with_secret(&secret, Date::now().as_millis() as i64 / 1000)
}

pub fn is_admin(req: &Request, env: &Env, password_hash: &str) -> bool {
    let Some(secret) = signing_secret(env, password_hash) else {
        return false;
    };
    let now = Date::now().as_millis() as i64 / 1000;
    let bearer_valid = req
        .headers()
        .get("Authorization")
        .ok()
        .flatten()
        .is_some_and(|header| {
            header
                .strip_prefix("Bearer ")
                .is_some_and(|token| verify_admin_jwt(token, &secret, now))
        });
    if bearer_valid {
        return true;
    }
    req.headers()
        .get("Cookie")
        .ok()
        .flatten()
        .is_some_and(|cookies| {
            cookies
                .split(';')
                .find_map(|entry| {
                    let (name, value) = entry.trim().split_once('=')?;
                    (name == "nodeflare_admin").then_some(value)
                })
                .is_some_and(|token| verify_admin_jwt(token, &secret, now))
        })
}

pub fn create_turnstile_proof(env: &Env, password_hash: &str) -> Option<String> {
    let secret = signing_secret(env, password_hash)?;
    let expires = Date::now().as_millis() as i64 / 1000 + TURNSTILE_PROOF_SECONDS;
    let signature = sha256_hex(&format!("nodeflare-turnstile:{expires}:{secret}"));
    Some(format!("{expires}.{signature}"))
}

pub fn verify_turnstile_proof(value: &str, env: &Env, password_hash: &str) -> bool {
    let Some(secret) = signing_secret(env, password_hash) else {
        return false;
    };
    let Some((expires_raw, signature)) = value.split_once('.') else {
        return false;
    };
    let Ok(expires) = expires_raw.parse::<i64>() else {
        return false;
    };
    let current = Date::now().as_millis() as i64 / 1000;
    if expires < current || expires > current + TURNSTILE_PROOF_SECONDS + 60 {
        return false;
    }
    let expected = sha256_hex(&format!("nodeflare-turnstile:{expires}:{secret}"));
    secure_eq(&expected, signature)
}

pub fn create_theme_preview_proof(
    env: &Env,
    password_hash: &str,
    theme_id: &str,
) -> Option<String> {
    let secret = signing_secret(env, password_hash)?;
    let expires = Date::now().as_millis() as i64 / 1000 + THEME_PREVIEW_SECONDS;
    let payload = format!("{theme_id}.{expires}");
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).ok()?;
    mac.update(format!("nodeflare-theme-preview:{payload}").as_bytes());
    let signature = URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());
    Some(format!("{payload}.{signature}"))
}

pub fn verify_theme_preview_proof(value: &str, env: &Env, password_hash: &str) -> Option<String> {
    let secret = signing_secret(env, password_hash)?;
    let mut parts = value.split('.');
    let (Some(theme_id), Some(expires), Some(signature), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return None;
    };
    if theme_id.is_empty() || theme_id.len() > 80 || theme_id.contains('/') {
        return None;
    }
    let expires = expires.parse::<i64>().ok()?;
    let current = Date::now().as_millis() as i64 / 1000;
    if expires < current || expires > current + THEME_PREVIEW_SECONDS + 60 {
        return None;
    }
    let signature = URL_SAFE_NO_PAD.decode(signature).ok()?;
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).ok()?;
    mac.update(format!("nodeflare-theme-preview:{theme_id}.{expires}").as_bytes());
    mac.verify_slice(&signature).ok()?;
    Some(theme_id.to_string())
}

pub fn bearer_token(req: &Request) -> Option<String> {
    req.headers()
        .get("Authorization")
        .ok()
        .flatten()
        .and_then(|value| value.strip_prefix("Bearer ").map(ToOwned::to_owned))
}

#[cfg(test)]
mod tests {
    use super::{
        create_admin_jwt_with_secret, hash_password, verify_admin_jwt, verify_password_hash,
        ADMIN_SESSION_SECONDS,
    };

    #[test]
    fn hashes_passwords_with_pbkdf2() {
        let encoded = hash_password(
            "correct horse battery staple",
            "0123456789abcdef0123456789abcdef",
        );
        assert!(encoded.starts_with("pbkdf2_sha256_client_v1$10000$"));
        assert!(verify_password_hash(
            "correct horse battery staple",
            &encoded
        ));
        assert!(!verify_password_hash("wrong", &encoded));
        assert!(!verify_password_hash(
            "correct horse battery staple",
            "sha256$0123456789abcdef0123456789abcdef$invalid"
        ));
    }

    #[test]
    fn signs_and_verifies_admin_jwt() {
        let issued_at = 1_800_000_000;
        let token = create_admin_jwt_with_secret("d1-password-hash", issued_at).expect("jwt");
        assert_eq!(token.split('.').count(), 3);
        assert!(verify_admin_jwt(&token, "d1-password-hash", issued_at + 1));
        assert!(verify_admin_jwt(
            &token,
            "d1-password-hash",
            issued_at + ADMIN_SESSION_SECONDS
        ));
        assert!(!verify_admin_jwt(
            &token,
            "d1-password-hash",
            issued_at + ADMIN_SESSION_SECONDS + 1
        ));
        assert!(!verify_admin_jwt(&token, "different-secret", issued_at));
    }

    #[test]
    fn rejects_tampered_admin_jwt() {
        let issued_at = 1_800_000_000;
        let token = create_admin_jwt_with_secret("d1-password-hash", issued_at).expect("jwt");
        let tampered = format!("{}x", token);
        assert!(!verify_admin_jwt(&tampered, "d1-password-hash", issued_at));
    }
}
