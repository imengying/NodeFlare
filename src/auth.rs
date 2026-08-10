use sha2::{Digest, Sha256};
use worker::{Date, Env, Request};

const SESSION_SECONDS: i64 = 7 * 24 * 60 * 60;
const TURNSTILE_PROOF_SECONDS: i64 = 60 * 60;
const THEME_PREVIEW_SECONDS: i64 = 10 * 60;
const PASSWORD_ROUNDS: usize = 10_000;

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

pub fn admin_username(env: &Env, configured: &str) -> String {
    if !configured.trim().is_empty() {
        return configured.trim().to_string();
    }
    env.var("ADMIN_USERNAME")
        .map(|value| value.to_string())
        .unwrap_or_else(|_| "admin".to_string())
}

fn password_digest(password: &str, salt: &str) -> String {
    let mut digest = Sha256::digest(format!("cf-monitor:{salt}:{password}").as_bytes()).to_vec();
    for _ in 1..PASSWORD_ROUNDS {
        let mut hasher = Sha256::new();
        hasher.update(&digest);
        hasher.update(salt.as_bytes());
        digest = hasher.finalize().to_vec();
    }
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub fn hash_password(password: &str, salt: &str) -> String {
    format!("sha256${salt}${}", password_digest(password, salt))
}

fn verify_password_hash(password: &str, encoded: &str) -> bool {
    let mut parts = encoded.split('$');
    let Some(algorithm) = parts.next() else {
        return false;
    };
    let Some(salt) = parts.next() else {
        return false;
    };
    let Some(expected) = parts.next() else {
        return false;
    };
    if algorithm != "sha256" || salt.is_empty() || parts.next().is_some() {
        return false;
    }
    secure_eq(&password_digest(password, salt), expected)
}

pub fn verify_credentials(
    env: &Env,
    configured_username: &str,
    password_hash: &str,
    username: &str,
    password: &str,
) -> bool {
    let expected_username = admin_username(env, configured_username);
    let username_valid = secure_eq(&expected_username, username.trim());
    let password_valid = if password_hash.is_empty() {
        admin_secret(env)
            .map(|expected| secure_eq(&expected, password))
            .unwrap_or(false)
    } else {
        verify_password_hash(password, password_hash)
    };
    username_valid && password_valid
}

fn signing_secret(env: &Env, password_hash: &str) -> Option<String> {
    if let Ok(secret) = env.secret("SESSION_SECRET") {
        return Some(secret.to_string());
    }
    if !password_hash.is_empty() {
        return Some(password_hash.to_string());
    }
    admin_secret(env)
}

pub fn create_session(env: &Env, password_hash: &str) -> Option<String> {
    let secret = signing_secret(env, password_hash)?;
    let expires = Date::now().as_millis() as i64 / 1000 + SESSION_SECONDS;
    let signature = sha256_hex(&format!("cf-monitor:{expires}:{secret}"));
    Some(format!("{expires}.{signature}"))
}

pub fn is_admin(req: &Request, env: &Env, password_hash: &str) -> bool {
    let Some(secret) = signing_secret(env, password_hash) else {
        return false;
    };
    let Ok(Some(header)) = req.headers().get("Authorization") else {
        return false;
    };
    let Some(token) = header.strip_prefix("Bearer ") else {
        return false;
    };
    let Some((expires_raw, signature)) = token.split_once('.') else {
        return false;
    };
    let Ok(expires) = expires_raw.parse::<i64>() else {
        return false;
    };
    let now = Date::now().as_millis() as i64 / 1000;
    if expires < now || expires > now + SESSION_SECONDS + 60 {
        return false;
    }
    let expected = sha256_hex(&format!("cf-monitor:{expires}:{secret}"));
    secure_eq(&expected, signature)
}

pub fn create_turnstile_proof(env: &Env, password_hash: &str) -> Option<String> {
    let secret = signing_secret(env, password_hash)?;
    let expires = Date::now().as_millis() as i64 / 1000 + TURNSTILE_PROOF_SECONDS;
    let signature = sha256_hex(&format!("cf-monitor-turnstile:{expires}:{secret}"));
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
    let expected = sha256_hex(&format!("cf-monitor-turnstile:{expires}:{secret}"));
    secure_eq(&expected, signature)
}

pub fn create_theme_preview(env: &Env, password_hash: &str, theme_url: &str) -> Option<String> {
    let secret = signing_secret(env, password_hash)?;
    let expires = Date::now().as_millis() as i64 / 1000 + THEME_PREVIEW_SECONDS;
    let theme_hash = sha256_hex(theme_url);
    let signature = sha256_hex(&format!("cf-monitor-theme:{expires}:{theme_hash}:{secret}"));
    Some(format!("{expires}.{signature}"))
}

pub fn verify_theme_preview(value: &str, env: &Env, password_hash: &str, theme_url: &str) -> bool {
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
    if expires < current || expires > current + THEME_PREVIEW_SECONDS + 60 {
        return false;
    }
    let theme_hash = sha256_hex(theme_url);
    let expected = sha256_hex(&format!("cf-monitor-theme:{expires}:{theme_hash}:{secret}"));
    secure_eq(&expected, signature)
}

pub fn bearer_token(req: &Request) -> Option<String> {
    req.headers()
        .get("Authorization")
        .ok()
        .flatten()
        .and_then(|value| value.strip_prefix("Bearer ").map(ToOwned::to_owned))
}
