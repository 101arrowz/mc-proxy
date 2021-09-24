use crate::protocol::{error::Error as ProtocolError, types::Chat};
use thiserror::Error;
use tokio::io::Error as IOError;
use reqwest::Error as HTTPError;

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
    #[error("HTTP error")]
    HTTPError(#[from] HTTPError),
    #[error("packet too big")]
    PacketTooBig(usize),
    #[error("invalid packet size")]
    InvalidPacketSize(i32),
}