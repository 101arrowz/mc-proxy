use super::error::Error as WebError;
use crate::protocol::types::{serde_raw_uuid, UUID};
use reqwest::{Client, StatusCode};
use serde::Deserialize;

#[derive(Clone, Debug)]
pub struct Mojang<'a> {
    access_token: Option<&'a str>,
    client: Client,
}

impl Mojang<'_> {
    pub fn new<'a>(access_token: Option<&'a str>, client: Option<Client>) -> Mojang<'a> {
        Mojang {
            access_token,
            client: client.unwrap_or_default(),
        }
    }

    pub async fn get_uuid(&self, name: &str) -> Result<(UUID, String), WebError> {
        #[derive(Deserialize)]
        struct UUIDResponse {
            #[serde(with = "serde_raw_uuid")]
            id: UUID,
            name: String
        }
        let res: UUIDResponse = self
            .client
            .get(["https://api.mojang.com/users/profiles/minecraft/", name].concat())
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok((res.id, res.name))
    }
}
