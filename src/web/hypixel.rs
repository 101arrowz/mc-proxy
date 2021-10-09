use super::error::Error as WebError;
use crate::protocol::types::{Chat, ChatObject, ChatValue, Color, UUID};
use reqwest::{Client, RequestBuilder};
use serde::{de, Deserialize, Deserializer};
use serde_json::{Map, Value};

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
        let map = Map::deserialize(deserializer)?;

        let success = map
            .get("success")
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
    #[serde(rename = "wins_bedwars")]
    pub wins: Option<u32>,
    #[serde(rename = "losses_bedwars")]
    pub losses: Option<u32>,
    pub winstreak: Option<u32>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct PlayerStats {
    #[serde(rename = "Bedwars")]
    pub bedwars: Option<PlayerBedwarsStats>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Rank {
    Default,
    VIP,
    VIPPlus,
    MVP,
    MVPPlus(Color),
    MVPPlusPlus(Color, Color),
    Youtuber,
    Admin,
    Custom(String),
}

#[derive(Clone, Debug)]
pub struct PlayerInfo {
    pub stats: PlayerStats,
    pub rank: Rank,
    pub name: String,
}

impl<'de> Deserialize<'de> for PlayerInfo {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let map = Map::deserialize(deserializer)?;

        let rank = if let Some(prefix) = map.get("prefix") {
            Rank::Custom(prefix.as_str().unwrap().into())
        } else {
            // TODO: Make this less godawful
            let rank = map
                .get("rank")
                .and_then(|rank| rank.as_str())
                .and_then(|v| if v == "NORMAL" { None } else { Some(v) })
                .or_else(|| {
                    map.get("monthlyPackageRank")
                        .and_then(|rank| rank.as_str())
                        .and_then(|v| if v == "NONE" { None } else { Some(v) })
                })
                .or_else(|| {
                    map.get("newPackageRank")
                        .and_then(|rank| rank.as_str())
                        .and_then(|v| if v == "NONE" { None } else { Some(v) })
                })
                .or_else(|| {
                    map.get("packageRank")
                        .and_then(|rank| rank.as_str())
                        .and_then(|v| if v == "NONE" { None } else { Some(v) })
                });
            if let Some(rank) = rank {
                match rank {
                    "ADMIN" => Rank::Admin,
                    "YOUTUBER" => Rank::Youtuber,
                    "SUPERSTAR" => Rank::MVPPlusPlus(
                        if let Some(color) =
                            map.get("monthlyRankColor").and_then(|rank| rank.as_str())
                        {
                            color
                                .to_ascii_lowercase()
                                .parse()
                                .map_err(de::Error::custom)?
                        } else {
                            Color::Gold
                        },
                        if let Some(plus_color) =
                            map.get("rankPlusColor").and_then(|rank| rank.as_str())
                        {
                            plus_color
                                .to_ascii_lowercase()
                                .parse()
                                .map_err(de::Error::custom)?
                        } else {
                            Color::Red
                        },
                    ),
                    "MVP_PLUS" => Rank::MVPPlus(
                        if let Some(plus_color) =
                            map.get("rankPlusColor").and_then(|rank| rank.as_str())
                        {
                            plus_color
                                .to_ascii_lowercase()
                                .parse()
                                .map_err(de::Error::custom)?
                        } else {
                            Color::Red
                        },
                    ),
                    "MVP" => Rank::MVP,
                    "VIP_PLUS" => Rank::VIPPlus,
                    "VIP" => Rank::VIP,
                    "NORMAL" | "NONE" | "DEFAULT" => Rank::Default,
                    _ => unreachable!(),
                }
            } else {
                Rank::Default
            }
        };

        Ok(PlayerInfo {
            stats: map
                .get("stats")
                .ok_or_else(|| de::Error::missing_field("stats"))
                .map(Deserialize::deserialize)?
                .map_err(de::Error::custom)?,
            name: map
                .get("displayname")
                .ok_or_else(|| de::Error::missing_field("displayname"))
                .map(Deserialize::deserialize)?
                .map_err(de::Error::custom)?,
            rank,
        })
    }
}

impl From<&PlayerInfo> for Chat<'static> {
    fn from(info: &PlayerInfo) -> Self {
        match &info.rank {
            &Rank::MVPPlus(plus_color) => Chat::Object(ChatObject {
                color: Some(Color::Aqua),
                value: ChatValue::Text {
                    text: "[MVP".into(),
                },
                extra: Some(vec![
                    Chat::Object(ChatObject {
                        color: Some(plus_color),
                        value: ChatValue::Text { text: "+".into() },
                        ..Default::default()
                    }),
                    Chat::Raw("] ".into()),
                    Chat::Raw(info.name.clone().into()),
                ]),
                ..Default::default()
            }),
            &Rank::MVPPlusPlus(color, plus_color) => Chat::Object(ChatObject {
                color: Some(color),
                value: ChatValue::Text {
                    text: "[MVP".into(),
                },
                extra: Some(vec![
                    Chat::Object(ChatObject {
                        color: Some(plus_color),
                        value: ChatValue::Text { text: "++".into() },
                        ..Default::default()
                    }),
                    Chat::Raw("] ".into()),
                    Chat::Raw(info.name.clone().into()),
                ]),
                ..Default::default()
            }),
            Rank::Default => Chat::Raw(["§7", &info.name].concat().into()),
            rank => Chat::Raw(
                [
                    match rank {
                        Rank::Admin => "§c[ADMIN]",
                        Rank::Youtuber => "§c[§fYOUTUBE§c]",
                        Rank::MVP => "§b[MVP]",
                        Rank::VIPPlus => "§a[VIP§6+§a]",
                        Rank::VIP => "§a[VIP]",
                        Rank::Custom(rank) => rank,
                        _ => unreachable!(),
                    },
                    " ",
                    &info.name,
                ]
                .concat()
                .into(),
            ),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
struct PlayerResponse {
    player: Option<PlayerInfo>,
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

    pub async fn info(&self, uuid: UUID) -> Result<Option<PlayerInfo>, WebError> {
        match self
            .with_auth(self.client.get("https://api.hypixel.net/player"))
            .query(&[("uuid", uuid)])
            .send()
            .await?
            .json::<HypixelResponse<PlayerResponse>>()
            .await?
        {
            HypixelResponse::Ok(PlayerResponse { player }) => Ok(player),
            HypixelResponse::Err(err) => Err(err)?,
        }
    }
}
