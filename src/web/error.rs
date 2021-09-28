use reqwest::Error as HTTPError;
use thiserror::Error;
use super::yggdrasil::Error as YggdrasilError;

#[derive(Debug, Error)]
pub enum Error {
    #[error("HTTP error")]
    HTTPError(#[from] HTTPError),
    #[error("Yggdrasil error")]
    YggdrasilError(#[from] YggdrasilError),
    #[error("no access token")]
    NoAccessToken
}
