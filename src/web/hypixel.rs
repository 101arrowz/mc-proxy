use crate::protocol::types::UUID;
use std::str::from_utf8_unchecked;
use super::error::Error as WebError;
use reqwest::{Client, RequestBuilder};
use serde::{Deserialize, Deserializer, de};
use serde_json::{Map, Value};

#[derive(Clone, Debug, Deserialize, thiserror::Error)]
#[error("{cause:?}")]
pub struct Error {
    cause: String,
    throttle: Option<bool>
}

#[derive(Clone, Debug)]
enum HypixelResponse<T> {
    Ok(T),
    Err(Error)
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for HypixelResponse<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let mut map = Map::deserialize(deserializer)?;

        let success = map.remove("success")
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
struct PlayerBedwarsStats {
    #[serde(rename = "final_kills_bedwars")]
    final_kills: Option<u32>,
    #[serde(rename = "final_deaths_bedwars")]
    final_deaths: Option<u32>
}

#[derive(Clone, Debug, Deserialize)]
pub struct PlayerStats {
    #[serde(rename = "Bedwars")]
    bedwars: Option<PlayerBedwarsStats>
}

#[derive(Clone, Debug, Deserialize)]
pub struct PlayerInfo {
    stats: PlayerStats
}

#[derive(Clone, Debug)]
pub struct Hypixel<'a> {
    api_key: &'a str,
    client: Client
}

impl Hypixel<'_> {
    pub fn new<'a>(api_key: &'a str, client: Option<Client>) -> Hypixel<'a> {
        Hypixel {
            api_key,
            client: client.unwrap_or_default()
        }
    }

    fn with_auth(&self, builder: RequestBuilder) -> RequestBuilder {
        builder.header("API-Key", self.api_key)
    }

    pub async fn info(&self, uuid: UUID) -> Result<PlayerInfo, WebError> {
        match self.with_auth(self.client.get("https://api.hypixel.net/player"))
            .query(&[("uuid", uuid)])
            .send().await?
            .json().await?
        {
            HypixelResponse::Ok(val) => Ok(val),
            HypixelResponse::Err(err) => Err(err)?
        }
    }
}
