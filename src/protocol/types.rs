use super::{error::Error, version::ProtocolVersion};
use serde::{
    de::{self, Visitor},
    Deserialize, Deserializer, Serialize, Serializer,
};
use std::{
    borrow::Cow,
    convert::{TryFrom, TryInto},
    fmt::{self, Display},
    future::Future,
    str::{from_utf8, from_utf8_unchecked, FromStr},
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn handle_io_err(err: std::io::Error) -> Error {
    match err.kind() {
        std::io::ErrorKind::UnexpectedEof => Error::UnexpectedEof,
        std::io::ErrorKind::WriteZero => Error::NeedMore,
        _ => todo!(),
    }
}

pub trait Decode<'a, R: AsyncReadExt + Unpin + 'a>: Sized {
    type Output: Future<Output = Result<Self, Error>> + 'a;

    fn decode(src: &'a mut R, version: ProtocolVersion) -> Self::Output;
}

pub trait Encode<'a, W: AsyncWriteExt + Unpin + 'a> {
    type Output: Future<Output = Result<(), Error>> + 'a;

    fn encode(self, tgt: &'a mut W, version: ProtocolVersion) -> Self::Output;
}

macro_rules! decode_inner_impl {
    ($lifetime:lifetime, $reader:ident, $src:ident, $version:ident, $body:expr) => {
        type Output = impl std::future::Future<Output = Result<Self, $crate::protocol::error::Error>> + $lifetime;

        fn decode($src: &$lifetime mut $reader, $version: $crate::protocol::version::ProtocolVersion) -> Self::Output {
            async move { $body }
        }
    };
    ($lifetime:lifetime, $reader:ident, $src:ident, $body:expr) => {
        decode_inner_impl!($lifetime, $reader, $src, _version, $body);
    };
}
pub(crate) use decode_inner_impl;

macro_rules! decode_impl {
    ($decode:ty, $src:ident, $version:ident, $body:expr) => {
        impl<'a, R: AsyncReadExt + Unpin + 'a> Decode<'a, R> for $decode {
            decode_inner_impl!('a, R, $src, $version, $body);
        }
    };
    ($decode:ty, $src:ident, $body:expr) => {
        decode_impl!($decode, $src, _version, $body);
    };
}
pub(crate) use decode_impl;

macro_rules! encode_inner_impl {
    ($lifetime:lifetime, $writer:ident, $self:ident, $tgt:ident, $version:ident, $body:expr) => {
        type Output = impl std::future::Future<Output = Result<(), $crate::protocol::error::Error>> + $lifetime;

        #[allow(unused_mut)]
        fn encode(mut $self, $tgt: &$lifetime mut $writer, $version: $crate::protocol::version::ProtocolVersion) -> Self::Output {
            async move { $body }
        }
    };
    ($lifetime:lifetime, $writer:ident, $self:ident, $tgt:ident, $body:expr) => {
        encode_inner_impl!($lifetime, $writer, $self, $tgt, _version, $body);
    };
}
pub(crate) use encode_inner_impl;

macro_rules! encode_impl {
    ($encode:ty, $self:ident, $tgt:ident, $version:ident, $body:expr) => {
        impl<'a, W: AsyncWriteExt + Unpin + 'a> Encode<'a, W> for $encode {
            encode_inner_impl!('a, W, $self, $tgt, $version, $body);
        }
    };
    ($encode:ty, $self:ident, $tgt:ident, $body:expr) => {
        encode_impl!($encode, $self, $tgt, _version, $body);
    };
}
pub(crate) use encode_impl;

macro_rules! num_impl {
    ($($type:ty, $read_fn:tt, $write_fn:tt),* $(,)?) => {
        $(
            decode_impl!($type, src, {
                src.$read_fn().await.map_err(handle_io_err)
            });
            encode_impl!($type, self, tgt, {
                tgt.$write_fn(self).await.map_err(handle_io_err)
            });
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

decode_impl!(bool, src, version, {
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

encode_impl!(bool, self, tgt, version, {
    (self as u8).encode(tgt, version).await
});

#[derive(Copy, Clone, Debug)]
pub struct VarInt(pub i32);

decode_impl!(VarInt, src, {
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
            Err(err) => return Err(handle_io_err(err)),
        }
    }
});

// TODO: get same performance with better code
encode_impl!(VarInt, self, tgt, {
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
    .map_err(handle_io_err)
});

impl VarInt {
    pub fn len(&self) -> usize {
        match self.0 {
            0..=127 => 1,
            128..=16383 => 2,
            16384..=2097151 => 3,
            2097152..=268435455 => 4,
            _ => 5
        } 
    }
}

impl From<i32> for VarInt {
    fn from(value: i32) -> Self {
        VarInt(value)
    }
}

impl From<VarInt> for i32 {
    fn from(value: VarInt) -> Self {
        value.0
    }
}

#[derive(Copy, Clone, Debug)]
pub struct VarLong(pub i64);

decode_impl!(VarLong, src, {
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
            Err(err) => return Err(handle_io_err(err)),
        }
    }
});

// TODO: get same performance with better code
encode_impl!(VarLong, self, tgt, {
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
    .map_err(handle_io_err)
});

impl VarLong {
    pub fn len(&self) -> usize {
        match self.0 {
            0..=127 => 1,
            128..=16383 => 2,
            16384..=2097151 => 3,
            2097152..=268435455 => 4,
            268435456..=34359738367 => 5,
            34359738368..=4398046511103 => 6,
            4398046511104..=562949953421311 => 7,
            562949953421312..=72057594037927935 => 8,
            72057594037927936.. => 9,
            _ => 10
        } 
    }
}

impl From<i64> for VarLong {
    fn from(value: i64) -> Self {
        VarLong(value)
    }
}

impl From<VarLong> for i64 {
    fn from(value: VarLong) -> Self {
        value.0
    }
}
#[derive(Clone, Debug)]
pub struct LengthCappedString<'a, const L: usize>(pub Cow<'a, str>);

impl<'a, R: AsyncReadExt + Unpin + 'a, const L: usize> Decode<'a, R> for LengthCappedString<'a, L> {
    decode_inner_impl!('a, R, src, version, {
        let str_len = VarInt::decode(src, version).await?.0 as usize;
        if str_len > (L << 2) {
            return Err(Error::Malformed);
        }
        let mut buf = vec![0; str_len];
        match src.read_exact(&mut buf).await {
            Ok(_) => match String::from_utf8(buf) {
                Ok(str) => Ok(LengthCappedString(Cow::Owned(str))),
                Err(_) => todo!(),
            },
            Err(err) => Err(handle_io_err(err)),
        }
    });
}

impl<'a, W: AsyncWriteExt + Unpin + 'a, const L: usize> Encode<'a, W>
    for LengthCappedString<'a, L>
{
    encode_inner_impl!('a, W, self, tgt, version, {
        let str = self.0;
        if str.len() > (L << 2) || str.len() > i32::MAX as usize {
            Err(Error::Malformed)
        } else {
            VarInt(str.len() as i32).encode(tgt, version).await?;
            tgt.write_all(str.as_bytes())
                .await
                .map_err(handle_io_err)
        }
    });
}

impl<'a, const L: usize> TryFrom<String> for LengthCappedString<'a, L> {
    type Error = Error;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.len() > (L << 2) || value.len() > i32::MAX as usize {
            Err(Error::Malformed)
        } else {
            Ok(LengthCappedString(Cow::Owned(value)))
        }
    }
}

impl<'a, const L: usize> TryFrom<&'a str> for LengthCappedString<'a, L> {
    type Error = Error;

    fn try_from(value: &'a str) -> Result<Self, Self::Error> {
        if value.len() > (L << 2) || value.len() > i32::MAX as usize {
            Err(Error::Malformed)
        } else {
            Ok(LengthCappedString(Cow::Borrowed(value)))
        }
    }
}

impl<'a, const L: usize> Display for LengthCappedString<'a, L> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "snake_case", tag = "action", content = "value")]
pub enum ChatClickEvent {
    OpenUrl(String),
    RunCommand(String),
    TwitchUserInfo(String),
    SuggestCommand(String),
    ChangePage(usize),
    CopyToClipboard(String)
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "snake_case", tag = "action", content = "value")]
pub enum ChatHoverEvent {
    ShowText(Box<Chat>),
    ShowItem(String),
    ShowEntity(String)
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ChatObject {
    bold: Option<bool>,
    italic: Option<bool>,
    underlined: Option<bool>,
    strikethrough: Option<bool>,
    obfuscated: Option<bool>,
    color: Option<String>,
    insertion: Option<String>,
    click_event: Option<ChatClickEvent>,
    hover_event: Option<ChatHoverEvent>,
    extra: Option<Vec<Chat>>,
    text: String
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(untagged)]
pub enum Chat {
    Raw(String),
    Array(Vec<Chat>),
    Object(ChatObject)
}

decode_impl!(Chat, src, version, {
    serde_json::from_str(&LengthCappedString::<262144>::decode(src, version).await?.0).map_err(|_| Error::Malformed)
});

encode_impl!(Chat, self, tgt, version, {
    let chat: LengthCappedString<262144> = serde_json::to_string(&self).map_err(|_| Error::Malformed)?.try_into()?;
    chat.encode(tgt, version).await
});

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct UUID(pub u128);

impl UUID {
    fn to_ascii_bytes(&self) -> [u8; 32] {
        let mut buf = [0; 32];
        for i in 0..16 {
            let byte = (self.0 >> (i << 3)) as u8;
            let hex_a = byte & 15;
            buf[i << 1] = hex_a + (if hex_a < 10 { b'0' } else { b'A' - 10 });
            let hex_b = byte >> 4;
            buf[(i << 1) + 1] = hex_b + (if hex_b < 10 { b'0' } else { b'A' - 10 });
        }
        buf
    }

    fn to_ascii_bytes_hyphenated(&self) -> [u8; 36] {
        let mut buf = [b'-'; 36];
        for i in 0..16 {
            let byte = (self.0 >> (i << 3)) as u8;
            let index = match i {
                0..=3 => i << 1,
                4..=5 => (i << 1) + 1,
                6..=7 => (i << 1) + 2,
                8..=9 => (i << 1) + 3,
                _ => (i << 1) + 4,
            };
            let hex_a = byte & 15;
            buf[index] = hex_a + (if hex_a < 10 { b'0' } else { b'A' - 10 });
            let hex_b = byte >> 4;
            buf[index + 1] = hex_b + (if hex_b < 10 { b'0' } else { b'A' - 10 });
        }
        buf
    }
}

decode_impl!(UUID, src, {
    match src.read_u128().await {
        Ok(num) => Ok(UUID(num)),
        Err(err) => Err(handle_io_err(err)),
    }
});

encode_impl!(UUID, self, tgt, {
    tgt.write_u128(self.0).await.map_err(handle_io_err)
});

impl FromStr for UUID {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.len() == 32 || s.len() == 36 {
            let mut res = 0u128;
            for (ind, byte) in s.bytes().filter(|&v| v != b'-').enumerate() {
                res |= ((match byte {
                    b'0'..=b'9' => byte - b'0',
                    b'a'..=b'f' => byte - b'a' + 10,
                    b'A'..=b'F' => byte - b'A' + 10,
                    _ => return Err(Error::Malformed),
                }) as u128)
                    << (ind << 2);
            }
            Ok(UUID(res))
        } else {
            Err(Error::Malformed)
        }
    }
}

impl TryFrom<&str> for UUID {
    type Error = <Self as FromStr>::Err;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        value.parse()
    }
}

impl Display for UUID {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(unsafe { from_utf8_unchecked(&self.to_ascii_bytes_hyphenated()) })
    }
}

impl Serialize for UUID {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(unsafe { from_utf8_unchecked(&self.to_ascii_bytes_hyphenated()) })
    }
}

struct UUIDVisitor;

impl<'de> Visitor<'de> for UUIDVisitor {
    type Value = UUID;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a UUID")
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
        value.parse().map_err(|err| E::custom(err))
    }
}

impl<'de> Deserialize<'de> for UUID {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_str(UUIDVisitor)
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Position {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

decode_impl!(Position, src, version, {
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

encode_impl!(Position, self, tgt, {
    let num = ((self.x as u64 & 67108863) << 38)
        | ((self.z as u64 & 67108863) << 12)
        | (self.y as u64 & 4095);
    tgt.write_u64(num).await.map_err(handle_io_err)
});

mod tests {
    use std::io::Cursor;
    use tokio::test;

    use super::super::version::ProtocolVersion;
    use super::Decode;
    use super::Encode;
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
                VarInt::decode(&mut cursor, ProtocolVersion::V1_8_9)
                    .await
                    .unwrap()
                    .0,
                value
            );
            VarInt(value)
                .encode(&mut out, ProtocolVersion::V1_8_9)
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
                VarLong::decode(&mut cursor, ProtocolVersion::V1_8_9)
                    .await
                    .unwrap()
                    .0,
                value
            );
            VarLong(value)
                .encode(&mut out, ProtocolVersion::V1_8_9)
                .await
                .unwrap();
            assert_eq!(&out, bytes);
        }
    }
}
