use super::error::Error as WebError;
use crate::{
    connection::{
        error::Error as ConnectionError,
        packets::login::{Authenticator, LoginCredentials},
    },
    protocol::types::{serde_raw_uuid, UUID},
};
use reqwest::{Client};
use serde::{Deserialize, Serialize};
use std::{borrow::Cow, future::Future};

#[derive(Clone, Debug, Deserialize, thiserror::Error)]
#[error("msft todo")]
pub struct Error {}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename = "camelCase")]
pub struct UserInfo<'a> {
    pub name: Cow<'a, str>,
    #[serde(with = "serde_raw_uuid")]
    pub id: UUID,
}

#[derive(Clone, Debug)]
pub struct Authentication<'a> {
    access_token: Cow<'a, str>,
    refresh_token: Option<Cow<'a, str>>,
    mc_access_token: Option<String>,
    client: Client,
}

impl Authentication<'_> {
    pub fn new<'a>(
        access_token: Cow<'a, str>,
        refresh_token: Option<Cow<'a, str>>,
        client: Option<Client>,
    ) -> Authentication<'a> {
        Authentication {
            access_token,
            refresh_token,
            mc_access_token: None,
            client: client.unwrap_or_default(),
        }
    }

    pub async fn get_access_token(&mut self) -> Result<&str, WebError> {
        if self.mc_access_token.is_none() {
            #[derive(Debug, Clone, Serialize)]
            #[serde(rename_all = "PascalCase")]
            struct XboxLiveRequestProperties<'a> {
                auth_method: &'a str,
                site_name: &'a str,
                rps_ticket: &'a str
            }
            #[derive(Debug, Clone, Serialize)]
            #[serde(rename_all = "PascalCase")]
            struct XboxLiveRequest<'a> {
                properties: XboxLiveRequestProperties<'a>,
                relying_party: &'a str,
                token_type: &'a str
            }
            
            #[derive(Debug, Clone, Deserialize)]
            struct XboxLiveResponseDisplayClaim {
                uhs: String
            }

            #[derive(Debug, Clone, Deserialize)]
            struct XboxLiveResponseDisplayClaims {
                xui: Vec<XboxLiveResponseDisplayClaim>
            }

            #[derive(Debug, Clone, Deserialize)]
            #[serde(rename_all = "PascalCase")]
            struct XboxLiveResponse {
                token: String,
                display_claims: XboxLiveResponseDisplayClaims
            }

            // TODO: refresh logic

            let xbl_res: XboxLiveResponse = self.client
                .post("https://user.auth.xboxlive.com/user/authenticate")
                .header("Accept", "application/json")
                .json(&XboxLiveRequest {
                    properties: XboxLiveRequestProperties {
                        auth_method: "RPS",
                        site_name: "user.auth.xboxlive.com",
                        rps_ticket: &["d=", &self.access_token].concat()
                    },
                    relying_party: "http://auth.xboxlive.com",
                    token_type: "JWT"
                })
                .send()
                .await?
                .json()
                .await?;
            
            #[derive(Debug, Clone, Serialize)]
            #[serde(rename_all = "PascalCase")]
            struct XSTSRequestProperties<'a> {
                sandbox_id: &'a str,
                user_tokens: [&'a str; 1],
            }

            #[derive(Debug, Clone, Serialize)]
            #[serde(rename_all = "PascalCase")]
            struct XSTSRequest<'a> {
                properties: XSTSRequestProperties<'a>,
                relying_party: &'a str,
                token_type: &'a str
            }

            #[derive(Debug, Clone, Deserialize)]
            #[serde(rename_all = "PascalCase")]
            struct XSTSResponse {
                token: String,
            }

            let xsts_res: XSTSResponse = self.client
                .post("https://xsts.auth.xboxlive.com/xsts/authorize")
                .header("Accept", "application/json")
                .json(&XSTSRequest {
                    properties: XSTSRequestProperties {
                        sandbox_id: "RETAIL",
                        user_tokens: [&xbl_res.token]
                    },
                    relying_party: "rp://api.minecraftservices.com/",
                    token_type: "JWT"
                })
                .send()
                .await?
                .json()
                .await?;

            #[derive(Debug, Clone, Serialize)]
            #[serde(rename_all = "camelCase")]
            struct MinecraftRequest<'a> {
                identity_token: &'a str
            }

            #[derive(Debug, Clone, Deserialize)]
            struct MinecraftResponse {
                access_token: String,
            }

            let mc_res: MinecraftResponse = self.client
                .post("https://api.minecraftservices.com/authentication/login_with_xbox")
                .json(&MinecraftRequest {
                    identity_token: &["XBL3.0 x=", &xbl_res.display_claims.xui[0].uhs, ";", &xsts_res.token].concat()
                })
                .send()
                .await?
                .json()
                .await?;

            self.mc_access_token = Some(mc_res.access_token);
        }
        Ok(&self.mc_access_token.as_ref().unwrap())
    }

    pub async fn get_info(&mut self) -> Result<UserInfo<'static>, WebError> {
        self.get_access_token().await?;

        Ok(self.client
            .get("https://api.minecraftservices.com/minecraft/profile")
            .bearer_auth(self.mc_access_token.as_ref().unwrap())
            .send()
            .await?
            .json()
            .await?)
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
    type CredentialsOutput = impl Future<Output = Result<LoginCredentials, ConnectionError>>;

    fn username(&self) -> &str {
        &self.user_info.name
    }

    fn credentials(mut self) -> Self::CredentialsOutput {
        async move {
            Ok(LoginCredentials {
                uuid: self.user_info.id,
                access_token: self.auth.get_access_token().await?.to_string(),
            })
        }
    }
}
