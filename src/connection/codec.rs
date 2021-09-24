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
use std::{cmp::{max, min}, future::Future, io::{Cursor, IoSlice}, pin::Pin, task::{Context, Poll, ready}};
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
    pub len: usize,
    pub id: i32,
    pub content: IncomingInnerPacket<&'a mut Decryptor<R>>,
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
        let len = VarInt::decode(&mut self.conn, self.version).await?.0;
        if len > 2097151 {
            Err(Error::PacketTooBig(len as usize))
        } else if len < 0 {
            Err(Error::InvalidPacketSize(len))
        } else {
            let len = len as usize;
            let mut rest_of_packet = Limit::new(&mut self.conn, len);
            let mut id = VarInt::decode(&mut rest_of_packet, self.version).await?.0;
            let content = if self.compressed {
                let decompressed_size = id;
                id = VarInt::decode(&mut rest_of_packet, self.version).await?.0;
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

pub enum MaybeZlibVec {
    None,
    Some(ZlibEncoder<Vec<u8>>)
}


impl MaybeZlibVec {
    fn vec(&self) -> &Vec<u8> {
        match self {
            MaybeZlibVec::None => panic!("unexpected vec()"),
            MaybeZlibVec::Some(tgt) => tgt.get_ref()
        }
    }
}

impl AsyncWrite for MaybeZlibVec {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, io::Error>> {
        match self.get_mut() {
            MaybeZlibVec::None => Poll::Ready(Ok(buf.len())),
            MaybeZlibVec::Some(tgt) => Pin::new(tgt).poll_write(cx, buf)
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        match self.get_mut() {
            MaybeZlibVec::None => Poll::Ready(Ok(())),
            MaybeZlibVec::Some(tgt) => Pin::new(tgt).poll_flush(cx)
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        match self.get_mut() {
            MaybeZlibVec::None => Poll::Ready(Ok(())),
            MaybeZlibVec::Some(tgt) => Pin::new(tgt).poll_shutdown(cx)
        }
    }

    fn poll_write_vectored(self: Pin<&mut Self>, cx: &mut Context<'_>, bufs: &[IoSlice<'_>]) -> Poll<Result<usize, io::Error>> {
        match self.get_mut() {
            MaybeZlibVec::None => Poll::Ready(Ok(bufs.iter().map(|v| v.len()).sum())),
            MaybeZlibVec::Some(tgt) => Pin::new(tgt).poll_write_vectored(cx, bufs)
        }
    }
}

pub enum OutgoingInnerPacket<W: AsyncWriteExt + Unpin> {
    Normal(Limit<W>),
    UnknownLength {
        vec: Vec<u8>,
        version: ProtocolVersion,
        len: usize,
        shutting_down: bool,
        tgt: W
    },
    Compressed {
        vec: Limit<MaybeZlibVec>,
        cache: Vec<u8>,
        version: ProtocolVersion,
        len: usize,
        shutting_down: bool,
        tgt: W
    },
}

impl<W: AsyncWriteExt + Unpin> OutgoingInnerPacket<W> {
    async fn new(mut tgt: W, len: Option<usize>, threshold: Option<usize>, version: ProtocolVersion) -> Result<OutgoingInnerPacket<W>, Error> {
        if let Some(threshold) = threshold {
            if let Some(len) = len {
                if len > 2097151 {
                    Err(Error::PacketTooBig(len))
                } else if len > threshold {
                    Ok(OutgoingInnerPacket::Compressed {
                        vec: Limit::new(MaybeZlibVec::Some(ZlibEncoder::new(Vec::with_capacity(len >> 1))), len),
                        version,
                        len: 0,
                        cache: Vec::new(),
                        shutting_down: false,
                        tgt
                    })
                } else {
                    Ok(OutgoingInnerPacket::Compressed {
                        vec: Limit::new(MaybeZlibVec::None, len),
                        version,
                        len: 0,
                        cache: Vec::with_capacity(len + 1),
                        shutting_down: false,
                        tgt
                    })
                }
            } else {
                Ok(OutgoingInnerPacket::Compressed {
                    vec: Limit::new(MaybeZlibVec::Some(ZlibEncoder::new(Vec::new())), usize::MAX),
                    version,
                    len: 0,
                    cache: Vec::with_capacity(threshold + 1),
                    shutting_down: false,
                    tgt
                })
            }
        } else {
            if let Some(len) = len {
                if len > 2097151 {
                    Err(Error::PacketTooBig(len))
                } else {
                    VarInt(len as i32).encode(&mut tgt, version).await?;
                    Ok(OutgoingInnerPacket::Normal(Limit::new(tgt, len)))
                }
            } else {
                Ok(OutgoingInnerPacket::UnknownLength {
                    vec: Vec::new(),
                    version,
                    len: 0,
                    shutting_down: false,
                    tgt
                })
            }
        }
    }
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
            OutgoingInnerPacket::UnknownLength { vec,  len, shutting_down, .. } => {
                if *shutting_down {
                    Poll::Ready(Ok(0))
                } else {
                    *len += buf.len();
                    if *len > 2097151 {
                        Poll::Ready(Err(io::Error::new(io::ErrorKind::InvalidData, Error::PacketTooBig(*len))))
                    } else {
                        Pin::new(vec).poll_write(cx, buf)
                    }
                }
            },
            OutgoingInnerPacket::Compressed { vec, cache, len, shutting_down, .. } => {
                Poll::Ready(if *shutting_down { Ok(0) } else { match ready!(Pin::new(vec).poll_write(cx, buf)) {
                    Ok(bytes_written) => {
                        *len += bytes_written;
                        if *len > 2097151 {
                            Err(io::Error::new(io::ErrorKind::InvalidData, Error::PacketTooBig(*len)))
                        } else {
                            if cache.len() < cache.capacity() {
                                cache.extend_from_slice(&buf[..min(bytes_written, cache.capacity() - cache.len())]);
                            }
                            Ok(bytes_written)
                        }
                    },
                    Err(err) => Err(err)
                } })
            }
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        match self.get_mut() {
            OutgoingInnerPacket::Normal(writer) => Pin::new(writer).poll_flush(cx),
            OutgoingInnerPacket::Compressed { vec, shutting_down, .. } => {
                if *shutting_down {
                    Poll::Ready(Ok(()))
                } else {
                    Pin::new(vec).poll_flush(cx)
                }
            },
            _ => Poll::Ready(Ok(()))
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        match self.get_mut() {
            OutgoingInnerPacket::Normal(writer) => Pin::new(writer).poll_shutdown(cx),
            OutgoingInnerPacket::UnknownLength { vec, version, len, shutting_down, tgt } => {
                if !*shutting_down {
                    if *len > 2097151 {
                        return Poll::Ready(Err(io::Error::new(io::ErrorKind::Other, Error::PacketTooBig(*len))));
                    }
                    let mut encode_buf = [0u8; 5];
                    let mut encode_buf = Cursor::new(encode_buf.as_mut());
                    {
                        let mut len_encode = VarInt(*len as i32).encode(&mut encode_buf, *version);
                        let len_encode = unsafe { Pin::new_unchecked(&mut len_encode) };
                        if len_encode.poll(cx) == Poll::Pending {
                            unreachable!("VarInt encode on cursor was not immediately ready");
                        }
                    }
                    let end_pos = encode_buf.position() as usize;
                    if let Err(err) = ready!(Pin::new(&mut *tgt).poll_write(cx, &encode_buf.into_inner()[..end_pos])) {
                        return Poll::Ready(Err(err));
                    }
                    *shutting_down = true;
                }
                while *len != 0 {
                    match ready!(Pin::new(&mut *tgt).poll_write(cx, &vec[vec.len() - *len..])) {
                        Ok(bytes_written) => *len -= bytes_written,
                        Err(err) => return Poll::Ready(Err(err))
                    }
                }
                Pin::new(tgt).poll_shutdown(cx)
            }
            OutgoingInnerPacket::Compressed { vec, cache, version, len, shutting_down, tgt } => {
                ready!(Pin::new(&mut *vec).poll_flush(cx))?;
                let use_cache = cache.len() < cache.capacity();
                let compressed = if use_cache { cache } else { vec.get_ref().vec() };
                if !*shutting_down {
                    let uncompressed_len = VarInt(if use_cache {
                        *len = compressed.len();
                        0
                    } else {
                        *len as i32
                    });
                    let true_len = compressed.len() + uncompressed_len.len();
                    // Still works when use_cache == true b/c true_len == *len
                    let max_len = max(*len, true_len);
                    if max_len > 2097151 {
                        return Poll::Ready(Err(io::Error::new(io::ErrorKind::Other, Error::PacketTooBig(max_len))));
                    }
                    
                    let mut encode_buf = [0u8; 10];
                    let mut encode_buf = Cursor::new(encode_buf.as_mut());
                    {
                        let mut total_len_encode = VarInt(true_len as i32).encode(&mut encode_buf, *version);
                        let total_len_encode = unsafe { Pin::new_unchecked(&mut total_len_encode) };
                        if total_len_encode.poll(cx) == Poll::Pending {
                            unreachable!("VarInt encode on cursor was not immediately ready");
                        }
                    }
                    {
                        let mut len_encode = uncompressed_len.encode(&mut encode_buf, *version);
                        let len_encode = unsafe { Pin::new_unchecked(&mut len_encode) };
                        if len_encode.poll(cx) == Poll::Pending {
                            unreachable!("VarInt encode on cursor was not immediately ready");
                        }
                    }
                    let end_pos = encode_buf.position() as usize;
                    if let Err(err) = ready!(Pin::new(&mut *tgt).poll_write(cx, &encode_buf.into_inner()[..end_pos])) {
                        return Poll::Ready(Err(err));
                    }
                    *shutting_down = true;
                }
                while *len != 0 {
                    match ready!(Pin::new(&mut *tgt).poll_write(cx, &compressed[compressed.len() - *len..])) {
                        Ok(bytes_written) => *len -= bytes_written,
                        Err(err) => return Poll::Ready(Err(err))
                    }
                }
                Pin::new(tgt).poll_shutdown(cx)
            }
        }
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[IoSlice<'_>],
    ) -> Poll<Result<usize, io::Error>> {
        match self.get_mut() {
            OutgoingInnerPacket::Normal(writer) => Pin::new(writer).poll_write_vectored(cx, bufs),
            OutgoingInnerPacket::UnknownLength { vec, shutting_down,.. } => {
                if *shutting_down {
                    Poll::Ready(Ok(0))
                } else {
                    Pin::new(vec).poll_write_vectored(cx, bufs)
                }
            },
            OutgoingInnerPacket::Compressed { vec, shutting_down, .. } => {
                if *shutting_down {
                    Poll::Ready(Ok(0))
                } else {
                    Pin::new(vec).poll_write_vectored(cx, bufs)
                }
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
        len: Option<usize>,
    ) -> Result<OutgoingPacket<'_, W>, Error> {
        let id = VarInt(id);
        let mut packet = OutgoingInnerPacket::new(&mut self.conn, len.map(|s| s + id.len()), self.compress_threshold, self.version).await?;
        id.encode(&mut packet, self.version).await?;
        Ok(packet)
    }
}
