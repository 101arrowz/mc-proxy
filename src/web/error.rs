use super::hypixel::Error as HypixelError;
use super::yggdrasil::Error as YggdrasilError;
use reqwest::Error as HTTPError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("HTTP error")]
    HTTP(#[from] HTTPError),
    #[error("Yggdrasil error")]
    Yggdrasil(#[from] YggdrasilError),
    #[error("Hypixel error")]
    Hypixel(#[from] HypixelError),
    #[error("no access token")]
    NoAccessToken,
}
