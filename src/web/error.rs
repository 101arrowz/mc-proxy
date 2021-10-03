use super::hypixel::Error as HypixelError;
use super::yggdrasil::Error as YggdrasilError;
use reqwest::Error as HTTPError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("HTTP error")]
    HTTPError(#[from] HTTPError),
    #[error("Yggdrasil error")]
    YggdrasilError(#[from] YggdrasilError),
    #[error("Hypixel error")]
    HypixelError(#[from] HypixelError),
    #[error("no access token")]
    NoAccessToken,
}
