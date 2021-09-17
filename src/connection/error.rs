use crate::protocol::error::Error as ProtocolError;
use thiserror::Error;
use tokio::io::Error as IoError;

#[derive(Debug, Error)]
pub enum Error {
    #[error("invalid target")]
    InvalidTarget,
    #[error("I/O error")]
    IoError(#[from] IoError),
    #[error("invalid protocol state")]
    InvalidState,
    #[error("protocol error")]
    ProtocolError(#[from] ProtocolError),
    #[error("packet too big")]
    PacketTooBig(usize),
    #[error("invalid packet size")]
    InvalidPacketSize(i32),
}
