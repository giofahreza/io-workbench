fn env_path(key: &str) -> Option<PathBuf> {
    env::var(key)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(PathBuf::from)
}

fn env_bool(key: &str, default: bool) -> bool {
    env::var(key)
        .ok()
        .and_then(|value| match value.to_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => Some(true),
            "0" | "false" | "no" | "off" => Some(false),
            _ => None,
        })
        .unwrap_or(default)
}

fn env_usize(key: &str, default: usize) -> usize {
    env::var(key)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn env_u64(key: &str, default: u64) -> u64 {
    env::var(key)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn validate_credentials(username: &str, password: &str) -> Result<()> {
    let username = username.trim();
    if username.is_empty() || password.is_empty() {
        return Err(CoreError::InvalidInput(
            "username and password are required".to_string(),
        ));
    }
    if username.chars().count() < 3 {
        return Err(CoreError::InvalidInput(
            "username must be at least 3 characters".to_string(),
        ));
    }
    if password.chars().count() < 6 {
        return Err(CoreError::InvalidInput(
            "password must be at least 6 characters".to_string(),
        ));
    }
    Ok(())
}

fn verify_totp(secret: &str, code: &str) -> Result<bool> {
    let code = code.trim();
    if code.len() != 6 || !code.bytes().all(|byte| byte.is_ascii_digit()) {
        return Ok(false);
    }
    let expected = code
        .parse::<u32>()
        .map_err(|_| CoreError::AuthenticationFailed)?;
    let secret = decode_base32_secret(secret)?;
    let now = Utc::now().timestamp().max(0) as u64 / 30;
    for offset in [-1_i64, 0, 1] {
        let counter = if offset < 0 {
            now.saturating_sub(offset.unsigned_abs())
        } else {
            now.saturating_add(offset as u64)
        };
        if hotp(&secret, counter)? == expected {
            return Ok(true);
        }
    }
    Ok(false)
}

fn hotp(secret: &[u8], counter: u64) -> Result<u32> {
    let mut mac = HmacSha1::new_from_slice(secret)
        .map_err(|_| CoreError::InvalidInput("invalid OTP secret".to_string()))?;
    mac.update(&counter.to_be_bytes());
    let digest = mac.finalize().into_bytes();
    let offset = (digest[19] & 0x0f) as usize;
    let binary = (((digest[offset] & 0x7f) as u32) << 24)
        | ((digest[offset + 1] as u32) << 16)
        | ((digest[offset + 2] as u32) << 8)
        | (digest[offset + 3] as u32);
    Ok(binary % 1_000_000)
}

fn decode_base32_secret(secret: &str) -> Result<Vec<u8>> {
    let normalized: String = secret
        .trim()
        .trim_start_matches("otpauth://totp/")
        .chars()
        .filter(|char| !char.is_whitespace() && *char != '-' && *char != '=')
        .map(|char| char.to_ascii_uppercase())
        .collect();
    if normalized.is_empty() {
        return Err(CoreError::InvalidInput(
            "IO_WORKBENCH_OTP_SECRET must not be empty".to_string(),
        ));
    }

    let mut bits: u32 = 0;
    let mut bit_count: u8 = 0;
    let mut output = Vec::with_capacity(normalized.len() * 5 / 8);
    for byte in normalized.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'2'..=b'7' => byte - b'2' + 26,
            _ => {
                return Err(CoreError::InvalidInput(
                    "IO_WORKBENCH_OTP_SECRET must be a valid Base32 secret".to_string(),
                ));
            }
        };
        bits = (bits << 5) | value as u32;
        bit_count += 5;
        while bit_count >= 8 {
            bit_count -= 8;
            output.push(((bits >> bit_count) & 0xff) as u8);
        }
    }
    if output.len() < 10 {
        return Err(CoreError::InvalidInput(
            "IO_WORKBENCH_OTP_SECRET must decode to at least 10 bytes".to_string(),
        ));
    }
    Ok(output)
}

pub fn generate_secret_token(prefix: &str) -> String {
    format!(
        "{prefix}_{}{}",
        Uuid::new_v4().simple(),
        Uuid::new_v4().simple()
    )
}

pub fn hash_secret_token(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    hex::encode(digest)
}

fn user_to_profile(user: &iowb_storage::StoredUser) -> UserProfile {
    UserProfile {
        id: user.id.clone(),
        username: user.username.clone(),
        email: None,
        created_at: user.created_at,
    }
}
