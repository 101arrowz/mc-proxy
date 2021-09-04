use super::version::ProtocolVersion;
use std::{
    convert::{TryFrom, TryInto},
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

    fn decode<R: AsyncReadExt + Unpin>(
        src: &mut R,
        version: ProtocolVersion,
    ) -> Self::DecodeOutput<'_, R>;
    fn encode<'a, W: AsyncWriteExt + Unpin>(
        self,
        tgt: &'a mut W,
        version: ProtocolVersion,
    ) -> Self::EncodeOutput<'a, W>;
}

macro_rules! decode_impl {
    ($src:ident, $version:ident, $body:expr) => {
        type DecodeOutput<'a, R: 'a> = impl Future<Output = Result<Self, Error>> + 'a;

        fn decode<R: AsyncReadExt + Unpin>(
            $src: &mut R,
            $version: ProtocolVersion,
        ) -> Self::DecodeOutput<'_, R> {
            async move { $body }
        }
    };
    ($src:ident, $body:expr) => {
        decode_impl!($src, _version, $body);
    };
}

macro_rules! encode_impl {
    ($self:ident, $tgt:ident, $version:ident, $body:expr) => {
        type EncodeOutput<'a, W: 'a> = impl Future<Output = Result<(), Error>> + 'a;

        fn encode<W: AsyncWriteExt + Unpin>($self, $tgt: &mut W, $version: ProtocolVersion) -> Self::EncodeOutput<'_, W> {
            async move {
                $body
            }
        }
    };
    ($self:ident, $tgt:ident, $body:expr) => {
        encode_impl!($self, $tgt, _version, $body);
    }
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

impl MCType for bool {
    decode_impl!(src, version, {
        match u8::decode(src, version).await {
            Ok(num) => {
                if num <= 1 {
                    Ok(num != 0)
                } else {
                    Err(Error::Malformed)
                }
            }
            Err(err) => Err(err),
        }
    });

    encode_impl!(self, tgt, version, {
        (self as u8).encode(tgt, version).await
    });
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

    // TODO: get same performance with better code
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
                    // Need unsigned shift for negative values
                    ((self.0 as u32) >> 28) as u8,
                ])
                .await
            }
        }
        .map_err(handle_write_err)
    });
}

impl From<i32> for VarInt {
    fn from(val: i32) -> Self {
        VarInt(val)
    }
}

impl From<VarInt> for i32 {
    fn from(val: VarInt) -> Self {
        val.0
    }
}

#[derive(Copy, Clone, Debug)]
pub struct VarLong(pub i64);

impl MCType for VarLong {
    decode_impl!(src, {
        let mut bit_offset = 0;
        let mut result = 0;
        loop {
            if bit_offset == 70 {
                return Err(Error::Malformed);
            }
            match src.read_u8().await {
                Ok(num) => {
                    let val = (num & 127) as i64;
                    result |= val << bit_offset;
                    if num >> 7 == 0 {
                        return Ok(VarLong(result));
                    }
                    bit_offset += 7;
                }
                Err(err) => return Err(handle_read_err(err)),
            }
        }
    });

    // TODO: get same performance with better code
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
            268435456..=34359738367 => {
                tgt.write_all(&[
                    self.0 as u8 | 128,
                    (self.0 >> 7) as u8 | 128,
                    (self.0 >> 14) as u8 | 128,
                    (self.0 >> 21) as u8 | 128,
                    (self.0 >> 28) as u8,
                ])
                .await
            }
            34359738368..=4398046511103 => {
                tgt.write_all(&[
                    self.0 as u8 | 128,
                    (self.0 >> 7) as u8 | 128,
                    (self.0 >> 14) as u8 | 128,
                    (self.0 >> 21) as u8 | 128,
                    (self.0 >> 28) as u8 | 128,
                    (self.0 >> 35) as u8,
                ])
                .await
            }
            4398046511104..=562949953421311 => {
                tgt.write_all(&[
                    self.0 as u8 | 128,
                    (self.0 >> 7) as u8 | 128,
                    (self.0 >> 14) as u8 | 128,
                    (self.0 >> 21) as u8 | 128,
                    (self.0 >> 28) as u8 | 128,
                    (self.0 >> 35) as u8 | 128,
                    (self.0 >> 42) as u8,
                ])
                .await
            }
            562949953421312..=72057594037927935 => {
                tgt.write_all(&[
                    self.0 as u8 | 128,
                    (self.0 >> 7) as u8 | 128,
                    (self.0 >> 14) as u8 | 128,
                    (self.0 >> 21) as u8 | 128,
                    (self.0 >> 28) as u8 | 128,
                    (self.0 >> 35) as u8 | 128,
                    (self.0 >> 42) as u8 | 128,
                    (self.0 >> 49) as u8,
                ])
                .await
            }
            72057594037927936.. => {
                tgt.write_all(&[
                    self.0 as u8 | 128,
                    (self.0 >> 7) as u8 | 128,
                    (self.0 >> 14) as u8 | 128,
                    (self.0 >> 21) as u8 | 128,
                    (self.0 >> 28) as u8 | 128,
                    (self.0 >> 35) as u8 | 128,
                    (self.0 >> 42) as u8 | 128,
                    (self.0 >> 49) as u8 | 128,
                    (self.0 >> 56) as u8,
                ])
                .await
            }
            _ => {
                tgt.write_all(&[
                    self.0 as u8 | 128,
                    (self.0 >> 7) as u8 | 128,
                    (self.0 >> 14) as u8 | 128,
                    (self.0 >> 21) as u8 | 128,
                    (self.0 >> 28) as u8 | 128,
                    (self.0 >> 35) as u8 | 128,
                    (self.0 >> 42) as u8 | 128,
                    (self.0 >> 49) as u8 | 128,
                    (self.0 >> 56) as u8 | 128,
                    // No unsigned cast because always negative here
                    1, // this byte is useless but needed
                ])
                .await
            }
        }
        .map_err(handle_write_err)
    });
}

impl From<i64> for VarLong {
    fn from(val: i64) -> Self {
        VarLong(val)
    }
}

impl From<VarLong> for i64 {
    fn from(val: VarLong) -> Self {
        val.0
    }
}

pub struct LengthCappedString<const L: usize>(pub String);

impl<const L: usize> MCType for LengthCappedString<L> {
    decode_impl!(src, version, {
        let str_len = VarInt::decode(src, version).await?.0 as usize;
        if str_len > (L << 2) {
            return Err(Error::Malformed);
        }
        let mut buf = vec![0; str_len];
        match src.read_exact(&mut buf).await {
            Ok(_) => match String::from_utf8(buf) {
                Ok(str) => Ok(LengthCappedString(str)),
                Err(_) => todo!(),
            },
            Err(err) => Err(handle_write_err(err)),
        }
    });

    encode_impl!(self, tgt, version, {
        let str = self.0;
        if str.len() > (L << 2) || str.len() > i32::MAX as usize {
            Err(Error::Malformed)
        } else {
            VarInt(str.len() as i32).encode(tgt, version).await?;
            tgt.write_all(str.as_bytes())
                .await
                .map_err(handle_write_err)
        }
    });
}

impl<const L: usize> TryFrom<String> for LengthCappedString<L> {
    type Error = Error;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.len() > (L << 2) || value.len() > i32::MAX as usize {
            Err(Error::Malformed)
        } else {
            Ok(LengthCappedString(value))
        }
    }
}

impl<const L: usize> TryFrom<&str> for LengthCappedString<L> {
    type Error = Error;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        if value.len() > (L << 2) || value.len() > i32::MAX as usize {
            Err(Error::Malformed)
        } else {
            Ok(LengthCappedString(String::from(value)))
        }
    }
}

impl<const L: usize> FromStr for LengthCappedString<L> {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.try_into()
    }
}

impl<const L: usize> Display for LengthCappedString<L> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

pub type Chat = LengthCappedString<262144>;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
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
                _ => return Err(Error::Malformed),
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

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Position {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl MCType for Position {
    decode_impl!(src, version, {
        i64::decode(src, version).await.map(|num| {
            let (x, y, z) = if version >= ProtocolVersion::V1_14_4 {
                (
                    (num >> 38) as i32,
                    (num << 52 >> 52) as i32,
                    (num << 26 >> 38) as i32,
                )
            } else {
                (
                    (num >> 38) as i32,
                    (num << 12 >> 38) as i32,
                    (num << 38 >> 38) as i32,
                )
            };
            Position { x, y, z }
        })
    });

    encode_impl!(self, tgt, {
        let num = ((self.x as u64 & 67108863) << 38)
            | ((self.z as u64 & 67108863) << 12)
            | (self.y as u64 & 4095);
        tgt.write_u64(num).await.map_err(handle_write_err)
    });
}

mod tests {
    use std::io::Cursor;
    use tokio::test;

    use super::super::version::ProtocolVersion;
    use super::MCType;
    use super::Position;
    use super::VarInt;
    use super::VarLong;

    #[test]
    async fn varint() {
        let test_cases: [(i32, &[u8]); 10] = [
            (0, &[0x00]),
            (1, &[0x01]),
            (2, &[0x02]),
            (127, &[0x7f]),
            (128, &[0x80, 0x01]),
            (255, &[0xff, 0x01]),
            (2097151, &[0xff, 0xff, 0x7f]),
            (2147483647, &[0xff, 0xff, 0xff, 0xff, 0x07]),
            (-1, &[0xff, 0xff, 0xff, 0xff, 0x0f]),
            (-2147483648, &[0x80, 0x80, 0x80, 0x80, 0x08]),
        ];
        for (value, bytes) in test_cases {
            let mut cursor = Cursor::new(bytes);
            let mut out = Vec::new();
            assert_eq!(
                VarInt::decode(&mut cursor, ProtocolVersion::V1_8)
                    .await
                    .unwrap()
                    .0,
                value
            );
            VarInt(value)
                .encode(&mut out, ProtocolVersion::V1_8)
                .await
                .unwrap();
            assert_eq!(&out, bytes);
        }
    }

    #[test]
    async fn varlong() {
        let test_cases: [(i64, &[u8]); 11] = [
            (0, &[0x00]),
            (1, &[0x01]),
            (2, &[0x02]),
            (127, &[0x7f]),
            (128, &[0x80, 0x01]),
            (255, &[0xff, 0x01]),
            (2147483647, &[0xff, 0xff, 0xff, 0xff, 0x07]),
            (
                9223372036854775807,
                &[0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x7f],
            ),
            (
                -1,
                &[0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x01],
            ),
            (
                -2147483648,
                &[0x80, 0x80, 0x80, 0x80, 0xf8, 0xff, 0xff, 0xff, 0xff, 0x01],
            ),
            (
                -9223372036854775808,
                &[0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x01],
            ),
        ];
        for (value, bytes) in test_cases {
            let mut cursor = Cursor::new(bytes);
            let mut out = Vec::new();
            assert_eq!(
                VarLong::decode(&mut cursor, ProtocolVersion::V1_8)
                    .await
                    .unwrap()
                    .0,
                value
            );
            VarLong(value)
                .encode(&mut out, ProtocolVersion::V1_8)
                .await
                .unwrap();
            assert_eq!(&out, bytes);
        }
    }
}
