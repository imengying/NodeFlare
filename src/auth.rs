use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use worker::{Date, Env, Request};

const SESSION_SECONDS: i64 = 7 * 24 * 60 * 60;
const TURNSTILE_PROOF_SECONDS: i64 = 60 * 60;
const PASSWORD_ROUNDS: usize = 10_000;
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

pub fn admin_username(env: &Env, configured: &str) -> String {
    if !configured.trim().is_empty() {
        return configured.trim().to_string();
    }
    env.var("ADMIN_USERNAME")
        .map(|value| value.to_string())
        .unwrap_or_else(|_| "admin".to_string())
}

fn password_digest(password: &str, salt: &str) -> String {
    let mut digest = Sha256::digest(format!("nodeflare:{salt}:{password}").as_bytes()).to_vec();
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
            exp: issued_at + SESSION_SECONDS,
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
        && claims.exp == claims.iat + SESSION_SECONDS
}

pub fn create_admin_jwt(env: &Env, password_hash: &str) -> Option<String> {
    let secret = signing_secret(env, password_hash)?;
    create_admin_jwt_with_secret(&secret, Date::now().as_millis() as i64 / 1000)
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
    let now = Date::now().as_millis() as i64 / 1000;
    verify_admin_jwt(token, &secret, now)
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

pub fn bearer_token(req: &Request) -> Option<String> {
    req.headers()
        .get("Authorization")
        .ok()
        .flatten()
        .and_then(|value| value.strip_prefix("Bearer ").map(ToOwned::to_owned))
}

#[cfg(test)]
mod tests {
    use super::{create_admin_jwt_with_secret, verify_admin_jwt, SESSION_SECONDS};

    #[test]
    fn signs_and_verifies_admin_jwt() {
        let issued_at = 1_800_000_000;
        let token = create_admin_jwt_with_secret("d1-password-hash", issued_at).expect("jwt");
        assert_eq!(token.split('.').count(), 3);
        assert!(verify_admin_jwt(&token, "d1-password-hash", issued_at + 1));
        assert!(verify_admin_jwt(
            &token,
            "d1-password-hash",
            issued_at + SESSION_SECONDS
        ));
        assert!(!verify_admin_jwt(
            &token,
            "d1-password-hash",
            issued_at + SESSION_SECONDS + 1
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
