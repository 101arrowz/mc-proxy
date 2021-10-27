use super::error::Error as WebError;
use crate::{
    connection::{
        error::Error as ConnectionError,
        packets::login::{Authenticator, LoginCredentials},
    },
    protocol::types::{serde_raw_uuid, UUID},
};
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;
use std::{borrow::Cow, future::Future};

#[derive(Clone, Debug, Deserialize)]
pub enum ErrorKind {
    #[serde(rename = "ForbiddenOperationException")]
    ForbiddenOperation,
    #[serde(rename = "IllegalArgumentException")]
    IllegalArgument,
    #[serde(rename = "ResourceException")]
    Resource,
}

#[derive(Clone, Debug, Deserialize, thiserror::Error)]
#[error("{kind:?}: {message} (cause: {cause:?})")]
pub struct Error {
    #[serde(rename = "error")]
    pub kind: ErrorKind,
    #[serde(rename = "errorMessage")]
    pub message: String,
    pub cause: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UserInfo<'a> {
    pub name: Cow<'a, str>,
    #[serde(with = "serde_raw_uuid")]
    pub id: UUID,
}

#[derive(Clone, Debug)]
pub struct Authentication<'a> {
    client_token: Option<&'a str>,
    access_token: Option<Cow<'a, str>>,
    client: Client,
}

#[derive(Debug, Clone, Serialize)]
struct AuthenticationRequestAgent<'a> {
    name: &'a str,
    version: usize,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AuthenticationRequest<'a> {
    // There is an agent, but is Minecraft by default
    username: &'a str,
    password: &'a str,
    client_token: Option<&'a str>,
    agent: AuthenticationRequestAgent<'a>,
}

#[derive(Debug, Clone)]
pub struct AuthenticationResponse<'a> {
    pub user_info: UserInfo<'static>,
    pub access_token: &'a str,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LocalAuthenticationResponse<'a> {
    selected_profile: UserInfo<'a>,
    access_token: String,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ValidationRefreshRequest<'a> {
    access_token: &'a str,
    client_token: Option<&'a str>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct RefreshRequest<'a> {
    access_token: &'a str,
    client_token: Option<&'a str>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum RawAuthenticationResponse {
    Success(LocalAuthenticationResponse<'static>),
    Failure(Error),
}

const MINECRAFT_AGENT: AuthenticationRequestAgent = AuthenticationRequestAgent {
    name: "Minecraft",
    version: 1,
};

impl Authentication<'_> {
    pub fn new<'a>(
        client_token: Option<&'a str>,
        access_token: Option<Cow<'a, str>>,
        client: Option<Client>,
    ) -> Authentication<'a> {
        Authentication {
            client_token,
            access_token,
            client: client.unwrap_or_default(),
        }
    }

    pub async fn authenticate(
        &mut self,
        username: &str,
        password: &str,
    ) -> Result<AuthenticationResponse<'_>, WebError> {
        let res = self
            .client
            .post("https://authserver.mojang.com/authenticate")
            .json(&AuthenticationRequest {
                client_token: self.client_token,
                username,
                password,
                agent: MINECRAFT_AGENT,
            })
            .send()
            .await?
            .json()
            .await?;
        match res {
            RawAuthenticationResponse::Success(auth) => {
                self.access_token = Some(Cow::Owned(auth.access_token));
                Ok(AuthenticationResponse {
                    user_info: auth.selected_profile,
                    access_token: self.access_token.as_ref().unwrap(),
                })
            }
            RawAuthenticationResponse::Failure(error) => Err(error.into()),
        }
    }

    pub async fn refresh_access_token(&mut self) -> Result<AuthenticationResponse<'_>, WebError> {
        if let Some(access_token) = &self.access_token {
            let res = self
                .client
                .post("https://authserver.mojang.com/refresh")
                .json(&ValidationRefreshRequest {
                    client_token: self.client_token,
                    access_token,
                })
                .send()
                .await?
                .json()
                .await?;
            match res {
                RawAuthenticationResponse::Success(auth) => {
                    self.access_token = Some(Cow::Owned(auth.access_token));
                    Ok(AuthenticationResponse {
                        user_info: auth.selected_profile,
                        access_token: self.access_token.as_ref().unwrap(),
                    })
                }
                RawAuthenticationResponse::Failure(error) => Err(error.into()),
            }
        } else {
            Err(WebError::NoAccessToken)
        }
    }

    pub async fn get_access_token(&mut self) -> Result<Option<&str>, WebError> {
        if let Some(access_token) = self.access_token.take() {
            if self
                .client
                .post("https://authserver.mojang.com/validate")
                .json(&ValidationRefreshRequest {
                    client_token: self.client_token,
                    access_token: &access_token,
                })
                .send()
                .await?
                .status()
                == StatusCode::NO_CONTENT
            {
                self.access_token = Some(access_token);
                Ok(Some(self.access_token.as_ref().unwrap()))
            } else {
                self.refresh_access_token()
                    .await
                    .map(|resp| Some(resp.access_token))
            }
        } else {
            Ok(None)
        }
    }
}

pub struct OnlineMode<'a> {
    user_info: UserInfo<'a>,
    auth: Authentication<'a>,
}

impl OnlineMode<'_> {
    pub fn new<'a>(user_info: UserInfo<'a>, auth: Authentication<'a>) -> OnlineMode<'a> {
        OnlineMode { user_info, auth }
    }
}

impl Authenticator for OnlineMode<'_> {
    type CredentialsOutput<'a> =
        impl Future<Output = Result<LoginCredentials<'a>, ConnectionError>> + 'a;

    fn username(&mut self) -> &str {
        &self.user_info.name
    }

    fn credentials(&mut self) -> Self::CredentialsOutput<'_> {
        async move {
            if let Some(access_token) = self.auth.get_access_token().await? {
                Ok(LoginCredentials {
                    uuid: self.user_info.id,
                    access_token,
                })
            } else {
                Err(WebError::NoAccessToken.into())
            }
        }
    }
}
