use crate::protocol::{error::Error as ProtocolError, types::Chat};
use crate::web::{error::Error as WebError};
use reqwest::Error as HTTPError;
use thiserror::Error;
use tokio::io::Error as IOError;

#[derive(Debug, Error)]
pub enum Error {
    #[error("invalid target")]
    InvalidTarget,
    #[error("I/O error")]
    IOError(#[from] IOError),
    #[error("invalid protocol state")]
    InvalidState,
    #[error("protocol error")]
    ProtocolError(#[from] ProtocolError),
    #[error("disconnected")]
    Disconnected(Chat<'static>),
    #[error("no credentials")]
    NoCredentials,
    #[error("HTTP error")]
    HTTPError(#[from] HTTPError),
    #[error("web error")]
    WebError(#[from] WebError),
    #[error("packet too big")]
    PacketTooBig(usize),
    #[error("invalid packet size")]
    InvalidPacketSize(i32),
    #[error("incomplete packet")]
    IncompletePacket
}
