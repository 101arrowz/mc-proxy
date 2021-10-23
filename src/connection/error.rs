use crate::protocol::{error::Error as ProtocolError, types::Chat};
use crate::web::error::Error as WebError;
use reqwest::Error as HTTPError;
use thiserror::Error;
use tokio::io::Error as IOError;

#[derive(Debug, Error)]
pub enum Error {
    #[error("invalid target")]
    InvalidTarget,
    #[error("I/O error")]
    IO(#[from] IOError),
    #[error("invalid protocol state")]
    InvalidState,
    #[error("protocol error")]
    Protocol(#[from] ProtocolError),
    #[error("disconnected")]
    Disconnected(Box<Chat<'static>>),
    #[error("no credentials")]
    NoCredentials,
    #[error("HTTP error")]
    HTTP(#[from] HTTPError),
    #[error("web error")]
    Web(#[from] WebError),
    #[error("packet too big")]
    PacketTooBig(usize),
    #[error("invalid packet size")]
    InvalidPacketSize(i32),
    #[error("incomplete packet")]
    IncompletePacket,
}
