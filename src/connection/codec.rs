use super::{
    encryption::{Decryptor, Encryptor},
    error::Error,
    util::Limit,
};
use crate::protocol::{
    types::{Decode, Encode, VarInt},
    version::ProtocolVersion,
};
use async_compression::tokio::{bufread::ZlibDecoder, write::ZlibEncoder};
use std::{
    io::IoSlice,
    pin::Pin,
    task::{Context, Poll},
};
use tokio::io::{self, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader, ReadBuf};

pub struct InboundConnection<R: AsyncReadExt + Unpin> {
    conn: Decryptor<R>,
    version: ProtocolVersion,
    compressed: bool,
}

pub enum IncomingInnerPacket<R: AsyncReadExt + Unpin> {
    Normal(Limit<R>),
    Decompressed(ZlibDecoder<BufReader<Limit<R>>>),
}

impl<R: AsyncReadExt + Unpin> AsyncRead for IncomingInnerPacket<R> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.get_mut() {
            IncomingInnerPacket::Normal(reader) => Pin::new(reader).poll_read(cx, buf),
            IncomingInnerPacket::Decompressed(reader) => Pin::new(reader).poll_read(cx, buf),
        }
    }
}

pub struct IncomingPacket<'a, R: AsyncReadExt + Unpin> {
    len: usize,
    id: i32,
    content: IncomingInnerPacket<&'a mut Decryptor<R>>,
}

impl<R: AsyncReadExt + Unpin> InboundConnection<R> {
    pub fn new(reader: R, version: ProtocolVersion) -> InboundConnection<R> {
        InboundConnection {
            conn: Decryptor::new(reader),
            version,
            compressed: false,
        }
    }

    pub async fn next_packet(&mut self) -> Result<IncomingPacket<'_, R>, Error> {
        let len: i32 = VarInt::decode(&mut self.conn, self.version).await?.into();
        if len > 2097151 {
            Err(Error::PacketTooBig(len as usize))
        } else if len < 0 {
            Err(Error::InvalidPacketSize(len))
        } else {
            let len = len as usize;
            let mut rest_of_packet = Limit::new_read(&mut self.conn, len);
            let mut id = VarInt::decode(&mut rest_of_packet, self.version)
                .await?
                .into();
            let content = if self.compressed {
                let decompressed_size = id;
                id = VarInt::decode(&mut rest_of_packet, self.version)
                    .await?
                    .into();
                if decompressed_size == 0 {
                    IncomingInnerPacket::Normal(rest_of_packet)
                } else {
                    IncomingInnerPacket::Decompressed(ZlibDecoder::new(BufReader::new(
                        rest_of_packet,
                    )))
                }
            } else {
                IncomingInnerPacket::Normal(rest_of_packet)
            };
            Ok(IncomingPacket { len, id, content })
        }
    }
}

pub enum OutgoingInnerPacket<W: AsyncWriteExt + Unpin> {
    Normal(Limit<W>),
    Compressed(Limit<ZlibEncoder<W>>),
}

type OutgoingPacket<'a, W> = OutgoingInnerPacket<&'a mut Encryptor<W>>;

pub struct OutboundConnection<W: AsyncWriteExt + Unpin> {
    conn: Encryptor<W>,
    version: ProtocolVersion,
    compress_threshold: Option<usize>,
}

impl<W: AsyncWriteExt + Unpin> AsyncWrite for OutgoingInnerPacket<W> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, io::Error>> {
        match self.get_mut() {
            OutgoingInnerPacket::Normal(writer) => Pin::new(writer).poll_write(cx, buf),
            OutgoingInnerPacket::Compressed(writer) => Pin::new(writer).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        match self.get_mut() {
            OutgoingInnerPacket::Normal(writer) => Pin::new(writer).poll_flush(cx),
            OutgoingInnerPacket::Compressed(writer) => Pin::new(writer).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        match self.get_mut() {
            OutgoingInnerPacket::Normal(writer) => Pin::new(writer).poll_shutdown(cx),
            OutgoingInnerPacket::Compressed(writer) => Pin::new(writer).poll_shutdown(cx),
        }
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[IoSlice<'_>],
    ) -> Poll<Result<usize, io::Error>> {
        match self.get_mut() {
            OutgoingInnerPacket::Normal(writer) => Pin::new(writer).poll_write_vectored(cx, bufs),
            OutgoingInnerPacket::Compressed(writer) => {
                Pin::new(writer).poll_write_vectored(cx, bufs)
            }
        }
    }
}

impl<W: AsyncWriteExt + Unpin> OutboundConnection<W> {
    pub fn new(writer: W, version: ProtocolVersion) -> OutboundConnection<W> {
        OutboundConnection {
            conn: Encryptor::new(writer),
            version,
            compress_threshold: None,
        }
    }

    pub async fn create_packet(
        &mut self,
        id: i32,
        len: usize,
    ) -> Result<OutgoingPacket<'_, W>, Error> {
        if len > 2097151 {
            Err(Error::PacketTooBig(len))
        } else {
            VarInt(len as i32)
                .encode(&mut self.conn, self.version)
                .await?;
            VarInt(id).encode(&mut self.conn, self.version).await?;
            let packet = if self
                .compress_threshold
                .map(|threshold| len > threshold)
                .unwrap_or(false)
            {
                OutgoingInnerPacket::Compressed(Limit::new_write(
                    ZlibEncoder::new(&mut self.conn),
                    len,
                ))
            } else {
                OutgoingInnerPacket::Normal(Limit::new_write(&mut self.conn, len))
            };
            Ok(packet)
        }
    }
}
