use super::{
    encryption::{MCDecryptor, MCEncryptor},
    util::Limit,
};
use crate::protocol::{
    self,
    types::{MCDecode, MCEncode, VarInt},
    version::ProtocolVersion,
};
use async_compression::tokio::{bufread::ZlibDecoder, write::ZlibEncoder};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, Take};

pub enum State {
    Handshaking,
    Status,
    Login,
    Play,
}

pub struct InboundConnection<R: AsyncReadExt + Unpin> {
    conn: MCDecryptor<R>,
    version: ProtocolVersion,
    decompress_threshold: usize,
}

pub enum IncomingInnerPacket<'a, R: AsyncReadExt + Unpin> {
    Normal(Limit<'a, R>),
    Decompressed(ZlibDecoder<BufReader<Limit<'a, R>>>),
}

pub struct IncomingPacket<'a, R: AsyncReadExt + Unpin> {
    len: usize,
    id: i32,
    content: IncomingInnerPacket<'a, MCDecryptor<R>>,
}

impl<R: AsyncReadExt + Unpin> InboundConnection<R> {
    pub async fn next_packet(&mut self) -> Result<IncomingPacket<'_, R>, protocol::error::Error> {
        let len = VarInt::decode(&mut self.conn, self.version).await?.0 as usize;
        let mut rest_of_packet = Limit::new(&mut self.conn, len);
        let mut id = VarInt::decode(&mut rest_of_packet, self.version)
            .await?
            .into();
        let content = if self.decompress_threshold > 0 {
            let decompressed_size = id;
            id = VarInt::decode(&mut rest_of_packet, self.version)
                .await?
                .into();
            if decompressed_size == 0 {
                IncomingInnerPacket::Normal(rest_of_packet)
            } else {
                IncomingInnerPacket::Decompressed(ZlibDecoder::new(BufReader::new(rest_of_packet)))
            }
        } else {
            IncomingInnerPacket::Normal(rest_of_packet)
        };
        Ok(IncomingPacket { len, id, content })
    }
}

pub struct OutboundConnection<W: AsyncWriteExt + Unpin> {
    conn: MCEncryptor<W>,
    version: ProtocolVersion,
    compress_threshold: usize,
}
