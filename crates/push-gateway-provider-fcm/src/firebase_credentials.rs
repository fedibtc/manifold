use serde::Deserialize;

/// Parsed Firebase service-account credentials.
#[derive(Clone, Eq, PartialEq)]
pub struct FirebaseCredentials {
    project_id: String,
    client_email: String,
    private_key: String,
    token_uri: String,
}

impl std::fmt::Debug for FirebaseCredentials {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("FirebaseCredentials(<redacted>)")
    }
}

impl FirebaseCredentials {
    /// Parses Firebase service-account credentials from raw JSON.
    ///
    /// # Errors
    ///
    /// Returns a sanitized error if the JSON is malformed, is not a
    /// `service_account`, or omits required service-account fields.
    pub fn from_json(json: &str) -> Result<Self, FirebaseCredentialsError> {
        let raw: RawFirebaseCredentials =
            serde_json::from_str(json).map_err(|_| FirebaseCredentialsError::InvalidJson)?;
        if raw.kind.as_deref() != Some("service_account") {
            return Err(FirebaseCredentialsError::InvalidServiceAccount);
        }
        if raw.project_id.trim().is_empty()
            || raw.client_email.trim().is_empty()
            || raw.private_key.trim().is_empty()
        {
            return Err(FirebaseCredentialsError::MissingRequiredField);
        }

        Ok(Self {
            project_id: raw.project_id,
            client_email: raw.client_email,
            private_key: raw.private_key,
            token_uri: raw
                .token_uri
                .filter(|uri| !uri.trim().is_empty())
                .unwrap_or_else(|| "https://oauth2.googleapis.com/token".to_owned()),
        })
    }

    /// Returns the Firebase project id.
    #[must_use]
    pub fn project_id(&self) -> &str {
        &self.project_id
    }

    /// Returns the service-account email.
    #[must_use]
    pub fn client_email(&self) -> &str {
        &self.client_email
    }

    /// Returns the service-account private key PEM.
    #[must_use]
    pub(crate) fn private_key(&self) -> &str {
        &self.private_key
    }

    /// Returns the OAuth token endpoint URI.
    #[must_use]
    pub fn token_uri(&self) -> &str {
        &self.token_uri
    }
}

/// Sanitized Firebase credential parsing error.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FirebaseCredentialsError {
    /// The service-account JSON was not valid JSON.
    InvalidJson,
    /// The JSON was not a Firebase service account object.
    InvalidServiceAccount,
    /// A required service-account field was absent or empty.
    MissingRequiredField,
}

impl std::fmt::Display for FirebaseCredentialsError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::InvalidJson => "invalid Firebase service-account JSON",
            Self::InvalidServiceAccount => "invalid Firebase service-account credentials",
            Self::MissingRequiredField => "missing Firebase service-account field",
        })
    }
}

impl std::error::Error for FirebaseCredentialsError {}

#[derive(Deserialize)]
struct RawFirebaseCredentials {
    #[serde(rename = "type")]
    kind: Option<String>,
    project_id: String,
    client_email: String,
    private_key: String,
    token_uri: Option<String>,
}
