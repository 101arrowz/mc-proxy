use std::{
    fmt::{self, Display},
    future::Future,
    str::{from_utf8, FromStr},
};

use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Debug, PartialEq, Error)]
pub enum Error {
    #[error("unexpected end of input")]
    UnexpectedEof,
    #[error("output buffer too small")]
    NeedMore,
    #[error("malformed data")]
    Malformed,
}

pub trait MCType: Sized {
    type DecodeOutput<'a, R: 'a>: Future<Output = Result<Self, Error>> + 'a;
    type EncodeOutput<'a, W: 'a>: Future<Output = Result<(), Error>> + 'a;

    fn decode<R: AsyncReadExt + Unpin>(src: &mut R) -> Self::DecodeOutput<'_, R>;
    fn encode<'a, W: AsyncWriteExt + Unpin>(self, tgt: &'a mut W) -> Self::EncodeOutput<'a, W>;
}

macro_rules! decode_impl {
    ($src:ident, $body:expr) => {
        type DecodeOutput<'a, R: 'a> = impl Future<Output = Result<Self, Error>> + 'a;

        fn decode<R: AsyncReadExt + Unpin>($src: &mut R) -> Self::DecodeOutput<'_, R> {
            async move { $body }
        }
    };
}

macro_rules! encode_impl {
    ($self: ident, $tgt:ident, $body:expr) => {
        type EncodeOutput<'a, W: 'a> = impl Future<Output = Result<(), Error>> + 'a;

        fn encode<W: AsyncWriteExt + Unpin>($self, $tgt: &mut W) -> Self::EncodeOutput<'_, W> {
            async move {
                $body
            }
        }
    };
}

fn handle_read_err(err: std::io::Error) -> Error {
    if err.kind() == std::io::ErrorKind::UnexpectedEof {
        Error::UnexpectedEof
    } else {
        todo!();
    }
}

fn handle_write_err(err: std::io::Error) -> Error {
    if err.kind() == std::io::ErrorKind::WriteZero {
        Error::NeedMore
    } else {
        todo!();
    }
}

macro_rules! num_impl {
    ($($ty:ty, $read_fn:tt, $write_fn:tt),* $(,)?) => {
        $(
            impl MCType for $ty {
                decode_impl!(src, {
                    src.$read_fn().await.map_err(handle_read_err)
                });

                encode_impl!(self, tgt, {
                    tgt.$write_fn(self).await.map_err(handle_write_err)
                });
            }
        )*
    }
}

num_impl! {
    i8, read_i8, write_i8,
    u8, read_u8, write_u8,
    i16, read_i16, write_i16,
    u16, read_u16, write_u16,
    i32, read_i32, write_i32,
    i64, read_i64, write_i64,
    f32, read_f32, write_f32,
    f64, read_f64, write_f64
}

#[derive(Copy, Clone, Debug)]
pub struct VarInt(pub i32);

impl MCType for VarInt {
    decode_impl!(src, {
        let mut bit_offset = 0;
        let mut result = 0;
        loop {
            if bit_offset == 35 {
                return Err(Error::Malformed);
            }
            match src.read_u8().await {
                Ok(num) => {
                    let val = (num & 127) as i32;
                    result |= val << bit_offset;
                    if num >> 7 == 0 {
                        return Ok(VarInt(result));
                    }
                    bit_offset += 7;
                }
                Err(err) => return Err(handle_read_err(err)),
            }
        }
    });

    encode_impl!(self, tgt, {
        match self.0 {
            0..=127 => tgt.write_all(&[self.0 as u8]).await,
            128..=16383 => {
                tgt.write_all(&[self.0 as u8 | 128, (self.0 >> 7) as u8])
                    .await
            }
            16384..=2097151 => {
                tgt.write_all(&[
                    self.0 as u8 | 128,
                    (self.0 >> 7) as u8 | 128,
                    (self.0 >> 14) as u8,
                ])
                .await
            }
            2097152..=268435455 => {
                tgt.write_all(&[
                    self.0 as u8 | 128,
                    (self.0 >> 7) as u8 | 128,
                    (self.0 >> 14) as u8 | 128,
                    (self.0 >> 21) as u8,
                ])
                .await
            }
            _ => {
                tgt.write_all(&[
                    self.0 as u8 | 128,
                    (self.0 >> 7) as u8 | 128,
                    (self.0 >> 14) as u8 | 128,
                    (self.0 >> 21) as u8 | 128,
                    (self.0 >> 28) as u8,
                ])
                .await
            }
        }
        .map_err(handle_write_err)
    });
}

#[derive(Copy, Clone, Debug)]
pub struct UUID(pub u128);

impl MCType for UUID {
    decode_impl!(src, {
        match src.read_u128().await {
            Ok(num) => Ok(UUID(num)),
            Err(err) => Err(handle_read_err(err)),
        }
    });

    encode_impl!(self, tgt, {
        tgt.write_u128(self.0).await.map_err(handle_write_err)
    });
}

impl FromStr for UUID {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.len() != 32 {
            return Err(Error::Malformed);
        }
        let mut res = 0u128;
        for (ind, byte) in s.bytes().enumerate() {
            res |= ((match byte {
                b'0'..=b'9' => byte - b'0',
                b'a'..=b'f' => byte - b'a' + 10,
                b'A'..=b'F' => byte - b'A' + 10,
                _ => todo!(),
            }) as u128)
                << (ind << 2);
        }
        Ok(UUID(res))
    }
}

impl Display for UUID {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut buf = [0; 32];
        for i in 0..16 {
            let byte = (self.0 >> (i << 3)) as u8;
            let hex_a = byte & 15;
            buf[i << 1] = hex_a + (if hex_a < 10 { b'0' } else { b'A' - 10 });
            let hex_b = byte >> 4;
            buf[(i << 1) + 1] = hex_b + (if hex_b < 10 { b'0' } else { b'A' - 10 });
        }
        f.write_str(from_utf8(&buf).unwrap())
    }
}
