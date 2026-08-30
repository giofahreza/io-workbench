#[derive(Clone)]
pub struct AuthManager {
    config: Arc<AppConfig>,
    storage: Storage,
}

impl AuthManager {
    pub fn new(config: Arc<AppConfig>, storage: Storage) -> Self {
        Self { config, storage }
    }

    pub fn status(&self, token: Option<&str>) -> Result<AuthStatusResponse> {
        let has_users = self.has_configured_user()?;
        let auth_enabled = self.should_enforce_auth()?;
        let auth_mode = self.auth_mode(has_users);
        let user = match token {
            Some(token) if !token.trim().is_empty() => self.authenticate_token(Some(token))?,
            _ if !auth_enabled => Some(self.local_user()?),
            _ => None,
        };
        let authenticated = user.is_some();
        Ok(AuthStatusResponse {
            enabled: auth_enabled,
            authenticated,
            needs_setup: auth_mode == "setup",
            is_authenticated: authenticated,
            auth_mode: auth_mode.to_string(),
            user,
        })
    }

    pub fn register(&self, username: &str, password: &str) -> Result<AuthTokenResponse> {
        if self.config.otp_secret.is_some() || self.config.local_token.is_some() {
            return Err(CoreError::Forbidden(
                "setup is disabled while token or OTP auth is configured".to_string(),
            ));
        }
        validate_credentials(username, password)?;
        if self.has_configured_user()? {
            return Err(CoreError::Forbidden(
                "user already exists; io-workbench is currently single-user".to_string(),
            ));
        }

        let password_hash = hash(password, DEFAULT_COST)
            .map_err(|error| CoreError::PasswordHash(error.to_string()))?;
        let user = self.storage.create_user(
            &format!("user_{}", Uuid::new_v4().simple()),
            username.trim(),
            &password_hash,
        )?;
        self.issue_token(&user)
    }

    pub fn login(&self, username: &str, password: &str) -> Result<AuthTokenResponse> {
        if let Some(secret) = self.config.otp_secret.as_deref() {
            if !verify_totp(secret, password.trim())? {
                return Err(CoreError::AuthenticationFailed);
            }
            let user = self.ensure_local_user()?;
            self.storage.update_last_login(&user.id)?;
            return self.issue_token(&user);
        }

        if !self.has_configured_user()? && self.config.local_token.is_some() {
            let expected = self
                .config
                .local_token
                .as_deref()
                .ok_or(CoreError::AuthenticationFailed)?;
            if password != expected {
                return Err(CoreError::AuthenticationFailed);
            }
            return Ok(AuthTokenResponse {
                success: true,
                token: expected.to_string(),
                user: self.local_user()?,
            });
        }

        let user = self
            .storage
            .get_user_by_username(username.trim())?
            .ok_or(CoreError::AuthenticationFailed)?;

        let password_hashes = self.storage.get_user_password_hashes(&user.id)?;
        let password_ok = password_hashes
            .into_iter()
            .try_fold(false, |matched, hash| {
                if matched {
                    return Ok(true);
                }
                verify(password, &hash).map_err(|error| CoreError::PasswordHash(error.to_string()))
            })?;
        if !password_ok {
            return Err(CoreError::AuthenticationFailed);
        }

        self.storage.update_last_login(&user.id)?;
        self.issue_token(&user)
    }

    pub fn logout(&self, token: Option<&str>) -> Result<bool> {
        let Some(token) = token else {
            return Ok(false);
        };
        if self.config.local_token.as_deref() == Some(token) {
            return Ok(true);
        }
        self.storage
            .revoke_auth_token(&hash_secret_token(token))
            .map_err(CoreError::from)
    }

    pub fn authenticate_token(&self, token: Option<&str>) -> Result<Option<UserProfile>> {
        if !self.should_enforce_auth()? && token.is_none() {
            return Ok(Some(self.local_user()?));
        }

        let Some(token) = token.filter(|token| !token.trim().is_empty()) else {
            return Ok(None);
        };

        if self.config.local_token.as_deref() == Some(token) {
            return Ok(Some(
                self.storage
                    .get_first_user()?
                    .map(|user| user_to_profile(&user))
                    .unwrap_or(self.local_user()?),
            ));
        }

        Ok(self
            .storage
            .find_user_by_token_hash(&hash_secret_token(token))?
            .map(|user| user_to_profile(&user)))
    }

    pub fn should_enforce_auth(&self) -> Result<bool> {
        Ok(self.config.auth_required
            || self.config.local_token.is_some()
            || self.config.otp_secret.is_some())
    }

    pub fn require_user(&self, token: Option<&str>) -> Result<UserProfile> {
        self.authenticate_token(token)?
            .ok_or(CoreError::AuthenticationFailed)
    }

    fn issue_token(&self, user: &iowb_storage::StoredUser) -> Result<AuthTokenResponse> {
        let token = generate_secret_token("iowb");
        let expires_at = Utc::now() + chrono::Duration::days(7);
        self.storage
            .create_auth_token(&hash_secret_token(&token), &user.id, expires_at)?;

        Ok(AuthTokenResponse {
            success: true,
            token,
            user: user_to_profile(user),
        })
    }

    fn local_user(&self) -> Result<UserProfile> {
        Ok(user_to_profile(&self.ensure_local_user()?))
    }

    fn ensure_local_user(&self) -> Result<iowb_storage::StoredUser> {
        if let Some(user) = self.storage.get_user_by_id("local")? {
            return Ok(user);
        }

        let password_hash = hash(generate_secret_token("local").as_str(), DEFAULT_COST)
            .map_err(|error| CoreError::PasswordHash(error.to_string()))?;
        match self.storage.create_user("local", "local", &password_hash) {
            Ok(user) => Ok(user),
            Err(_) => self
                .storage
                .get_user_by_id("local")?
                .ok_or_else(|| CoreError::InvalidInput("failed to create local user".to_string())),
        }
    }

    fn has_configured_user(&self) -> Result<bool> {
        Ok(self.storage.has_non_local_user()?)
    }

    fn auth_mode(&self, has_users: bool) -> &'static str {
        if self.config.otp_secret.is_some() {
            "otp"
        } else if self.config.local_token.is_some() {
            "token"
        } else if self.config.auth_required && !has_users {
            "setup"
        } else if self.config.auth_required {
            "password"
        } else {
            "open"
        }
    }
}
