use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, Take};
use super::encryption::{MCDecryptor, MCEncryptor};
use crate::protocol::{self, types::{MCDecode, MCEncode, VarInt}, version::ProtocolVersion};
use async_compression::tokio::{bufread::ZlibDecoder, write::ZlibEncoder};

pub enum State {
    Handshaking,
    Status,
    Login,
    Play
}

pub struct InboundConnection<R: AsyncReadExt + Unpin> {
    conn: MCDecryptor<R>,
    version: ProtocolVersion,
    decompress_threshold: usize
}

struct Packet<R: AsyncReadExt + Unpin> {
    size: i32,
    id: i32,
    content: Take<R>
}

impl<R: AsyncReadExt + Unpin> InboundConnection<R> {
    pub async fn next_packet(&mut self) -> Result<Packet<R>, protocol::error::Error> {
        let size = VarInt::decode(&mut self.conn, self.version).await?.into();
        if self.decompress_threshold > 0 {
            
        }
        Ok(Packet {
            size
        })
    }
}

pub struct OutboundConnection<W: AsyncWriteExt + Unpin> {
    conn: MCEncryptor<W>,
    version: ProtocolVersion,
    compress_threshold: usize
}