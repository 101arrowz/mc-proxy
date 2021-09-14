use tokio::io::{self, AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, Take};

use crate::protocol::{
    error::handle_write_err,
    types::{mcdecode_inner_impl, mcencode_impl, mcencode_inner_impl, MCDecode, MCEncode, VarInt},
};
use async_compression::tokio::bufread::ZlibDecoder;

pub struct Packet<R: AsyncReadExt + Unpin> {
    pub len: i32,
    pub id: i32,
    pub content: R,
}

impl<'a, R: AsyncReadExt + Unpin + 'a> MCDecode<'a, R> for Packet<Take<&'a mut R>> {
    mcdecode_inner_impl!('a, R, src, version, {
        let len = VarInt::decode(src, version).await?.into();
        Ok(Packet {
            len,
            id: VarInt::decode(src, version).await?.into(),
            content: src.take(len as u64)
        })
    });
}

impl<'a, R: AsyncReadExt + Unpin + 'a, W: AsyncWriteExt + Unpin + 'a> MCEncode<'a, W>
    for Packet<R>
{
    mcencode_inner_impl!('a, W, self, tgt, version, {
        VarInt(self.len).encode(tgt, version).await?;
        match io::copy(&mut self.content, tgt).await {
            Ok(_) => Ok(()),
            Err(err) => Err(handle_write_err(err))
        }
    });
}
