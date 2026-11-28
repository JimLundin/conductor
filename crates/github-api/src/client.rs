use std::time::Duration;

use github::access::{AccessError, Repository, TokenKind};
use octocrab::{
    Octocrab,
    models::{AppId, InstallationId},
    service::middleware::retry::RetryConfig,
};
use rsa::{
    RsaPrivateKey, pkcs1::DecodeRsaPrivateKey as _,
    pkcs8::DecodePrivateKey as _,
};
use secrecy::{ExposeSecret as _, SecretString};

use crate::credential::Source;

pub struct Client {
    pub(crate) repository: Repository,
    pub(crate) endpoint: String,
    pub(crate) reader: Octocrab,
    pub(crate) kind: TokenKind,
    pub(crate) writes: bool,
    personal: Option<SecretString>,
}

impl Client {
    pub(crate) fn new(
        repository: Repository,
        source: &Source,
        endpoint: &str,
    ) -> Result<Self, AccessError> {
        let (builder, authentication) =
            authenticate(builder(endpoint)?, source)?;
        let api = builder.build().map_err(configuration)?;
        let (reader, kind, personal) = match authentication {
            Authentication::User(kind, token) => (api, kind, Some(token)),
            Authentication::Installation(id) => (
                api.installation_builder(id)
                    .repositories(vec![repository.name().to_owned()])
                    .build()
                    .map_err(configuration)?,
                TokenKind::Installation,
                None,
            ),
        };
        Ok(Self {
            endpoint: endpoint.to_owned(),
            repository,
            reader,
            kind,
            writes: false,
            personal,
        })
    }

    pub(crate) const fn writing(mut self, writes: bool) -> Self {
        self.writes = writes;
        self
    }

    async fn current_token(&self) -> Result<SecretString, AccessError> {
        match self.personal.as_ref() {
            Some(token) => Ok(token.clone()),
            None => self.reader.installation_token().await.map_err(classify),
        }
    }

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

enum Authentication {
    User(TokenKind, SecretString),
    Installation(InstallationId),
}

fn authenticate(
    builder: Builder,
    source: &Source,
) -> Result<(Builder, Authentication), AccessError> {
    match source {
        Source::FineGrained { token } => {
            user(builder, token, TokenKind::FineGrained)
        }
        Source::Classic { token } => user(builder, token, TokenKind::Classic),
        Source::OAuth { token } => user(builder, token, TokenKind::OAuth),
        Source::Installation {
            app_id,
            installation_id,
            private_key,
        } => {
            let pem = private_key.expose_secret();
            RsaPrivateKey::from_pkcs1_pem(pem)
                .or_else(|_| RsaPrivateKey::from_pkcs8_pem(pem))
                .map_err(|_| AccessError::InvalidConfiguration)?;
            let app = builder
                .app_from_rsa_pem(AppId(app_id.get()), pem.as_bytes())
                .map_err(configuration)?;
            Ok((
                app,
                Authentication::Installation(InstallationId(
                    installation_id.get(),
                )),
            ))
        }
        Source::SshAgent { .. } => Err(AccessError::InvalidConfiguration),
    }
}

fn user(
    builder: Builder,
    token: &SecretString,
    kind: TokenKind,
) -> Result<(Builder, Authentication), AccessError> {
    github::access::validate_bearer(token.expose_secret())?;
    let authenticated = if kind == TokenKind::OAuth {
        builder.user_access_token(token.clone())
    } else {
        builder.personal_token(token.clone())
    };
    Ok((authenticated, Authentication::User(kind, token.clone())))
}

pub fn validate(source: &Source) -> Result<(), AccessError> {
    authenticate(Octocrab::builder(), source).map(drop)
}

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

pub fn unsent(error: octocrab::Error) -> home::issue::SendFailure {
    let status = match &error {
        octocrab::Error::GitHub { source, .. } => {
            Some(source.status_code.as_u16())
        }
        _ => None,
    };
    github::access::send_failure(status, classify(error))
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
    AccessError::response(
        source.status_code.as_u16(),
        exhausted,
        retry_after || source.message.contains("secondary rate limit"),
    )
}
