use super::error::Error as WebError;
use crate::protocol::types::UUID;
use reqwest::{Client, RequestBuilder};
use serde::{de, Deserialize, Deserializer};
use serde_json::{Map, Value};
use std::str::from_utf8_unchecked;

#[derive(Clone, Debug, Deserialize, thiserror::Error)]
#[error("{cause:?}")]
pub struct Error {
    cause: String,
    throttle: Option<bool>,
}

#[derive(Clone, Debug)]
enum HypixelResponse<T> {
    Ok(T),
    Err(Error),
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for HypixelResponse<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let mut map = Map::deserialize(deserializer)?;

        let success = map
            .remove("success")
            .ok_or_else(|| de::Error::missing_field("success"))
            .map(Deserialize::deserialize)?
            .map_err(de::Error::custom)?;
        let rest = Value::Object(map);

        if success {
            T::deserialize(rest)
                .map(HypixelResponse::Ok)
                .map_err(de::Error::custom)
        } else {
            Error::deserialize(rest)
                .map(HypixelResponse::Err)
                .map_err(de::Error::custom)
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct PlayerBedwarsStats {
    #[serde(rename = "final_kills_bedwars")]
    pub final_kills: Option<u32>,
    #[serde(rename = "final_deaths_bedwars")]
    pub final_deaths: Option<u32>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct PlayerStats {
    #[serde(rename = "Bedwars")]
    pub bedwars: Option<PlayerBedwarsStats>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct PlayerInfo {
    pub stats: PlayerStats,
}

#[derive(Clone, Debug, Deserialize)]
struct PlayerResponse {
    player: PlayerInfo,
}

#[derive(Clone, Debug)]
pub struct Hypixel<'a> {
    api_key: &'a str,
    client: Client,
}

impl Hypixel<'_> {
    pub fn new<'a>(api_key: &'a str, client: Option<Client>) -> Hypixel<'a> {
        Hypixel {
            api_key,
            client: client.unwrap_or_default(),
        }
    }

    fn with_auth(&self, builder: RequestBuilder) -> RequestBuilder {
        builder.header("API-Key", self.api_key)
    }

    pub async fn info(&self, uuid: UUID) -> Result<PlayerInfo, WebError> {
        match self
            .with_auth(self.client.get("https://api.hypixel.net/player"))
            .query(&[(
                "uuid",
                std::str::from_utf8(&uuid.to_ascii_bytes_hyphenated()).unwrap(),
            )])
            .send()
            .await?
            .json()
            .await?
        {
            HypixelResponse::Ok(PlayerResponse { player }) => Ok(player),
            HypixelResponse::Err(err) => Err(err)?,
        }
    }
}
