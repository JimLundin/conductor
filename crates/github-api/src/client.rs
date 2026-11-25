use std::time::Duration;

use github::access::{AccessError, Repository, TokenKind};
use octocrab::{
    Octocrab,
    models::{AppId, InstallationId},
    service::middleware::retry::RetryConfig,
};
use secrecy::{ExposeSecret as _, SecretString};

use crate::credential::{Source, token};

pub struct Client {
    pub(crate) repository: Repository,
    pub(crate) reader: Octocrab,
    pub(crate) kind: TokenKind,
    personal: Option<SecretString>,
}

impl Client {
    pub(crate) fn new(
        repository: Repository,
        source: Source,
        endpoint: &str,
    ) -> Result<Self, AccessError> {
        let builder = builder(endpoint)?;
        let (reader, kind, personal) = match source {
            Source::FineGrained { token: secret } => {
                let value = token(secret)?;
                (
                    builder
                        .personal_token(value.clone())
                        .build()
                        .map_err(configuration)?,
                    TokenKind::FineGrained,
                    Some(value),
                )
            }
            Source::Classic { token: secret } => {
                let value = token(secret)?;
                (
                    builder
                        .personal_token(value.clone())
                        .build()
                        .map_err(configuration)?,
                    TokenKind::Classic,
                    Some(value),
                )
            }
            Source::Installation {
                app_id,
                installation_id,
                private_key,
            } => (
                builder
                    .app_from_rsa_pem(
                        AppId(app_id),
                        private_key.expose_secret().as_bytes(),
                    )
                    .map_err(configuration)?
                    .build()
                    .map_err(configuration)?
                    .installation_builder(InstallationId(installation_id))
                    .repositories(vec![repository.name().to_owned()])
                    .build()
                    .map_err(configuration)?,
                TokenKind::Installation,
                None,
            ),
            Source::SshAgent { .. } => {
                return Err(AccessError::InvalidConfiguration);
            }
        };
        Ok(Self {
            repository,
            reader,
            kind,
            personal,
        })
    }

    async fn current_token(&self) -> Result<SecretString, AccessError> {
        match self.personal.as_ref() {
            Some(token) => Ok(token.clone()),
            None => self.reader.installation_token().await.map_err(classify),
        }
    }

    /// Acquires the repository's current token before entering blocking Git.
    ///
    /// # Errors
    /// Installation authentication or token refresh failed.
    pub async fn git(
        &self,
    ) -> Result<project_runtime::transport::Credentials, AccessError> {
        Ok(project_runtime::transport::Credentials::https(
            self.repository.https_remote(),
            self.current_token().await?,
        ))
    }
}

type Builder = octocrab::OctocrabBuilder<
    octocrab::NoSvc,
    octocrab::DefaultOctocrabBuilderConfig,
    octocrab::NoAuth,
    octocrab::NotLayerReady,
>;

fn builder(endpoint: &str) -> Result<Builder, AccessError> {
    Ok(Octocrab::builder()
        .base_uri(endpoint)
        .map_err(configuration)?
        .set_read_timeout(Some(Duration::from_secs(15)))
        .set_connect_timeout(Some(Duration::from_secs(10)))
        .add_retry_config(RetryConfig::None))
}

fn configuration(_: octocrab::Error) -> AccessError {
    AccessError::InvalidConfiguration
}

pub fn classify(error: octocrab::Error) -> AccessError {
    let octocrab::Error::GitHub { source, .. } = error else {
        return AccessError::RequestFailed;
    };
    let headers = source.headers.as_ref();
    let exhausted = headers
        .and_then(|h| h.get("x-ratelimit-remaining"))
        .is_some_and(|value| value == "0");
    let retry_after = headers.is_some_and(|h| h.contains_key("retry-after"));
    AccessError::response(source.status_code.as_u16(), exhausted, retry_after)
}
