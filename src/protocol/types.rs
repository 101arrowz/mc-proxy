use super::{error::Error, version::ProtocolVersion};
use serde::{Deserialize, Serialize};
use serde_with::{SerializeDisplay, DeserializeFromStr, skip_serializing_none};
use std::{
    borrow::Cow,
    convert::{TryFrom, TryInto},
    fmt::{self, Display},
    future::Future,
    str::{from_utf8_unchecked, FromStr},
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
            _ => 5,
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
            _ => 10,
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

impl<'a, R: AsyncReadExt + Unpin + 'a, const L: usize> Decode<'a, R> for LengthCappedString<'static, L> {
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

const BLACK: &str = "black";
const DARK_BLUE: &str = "dark_blue";
const DARK_GREEN: &str = "dark_green";
const DARK_AQUA: &str = "dark_aqua";
const DARK_RED: &str = "dark_red";
const DARK_PURPLE: &str = "dark_purple";
const GOLD: &str = "gold";
const GRAY: &str = "gray";
const DARK_GRAY: &str = "dark_gray";
const BLUE: &str = "blue";
const GREEN: &str = "green";
const AQUA: &str = "aqua";
const RED: &str = "red";
const LIGHT_PURPLE: &str = "light_purple";
const YELLOW: &str = "yellow";
const WHITE: &str = "white";
const RESET: &str = "reset";

#[derive(Clone, Debug, PartialEq, Eq, SerializeDisplay, DeserializeFromStr)]
pub enum Color {
    Black,
    DarkBlue,
    DarkGreen,
    DarkAqua,
    DarkRed,
    DarkPurple,
    Gold,
    Gray,
    DarkGray,
    Blue,
    Green,
    Aqua,
    Red,
    LightPurple,
    Yellow,
    White,
    Reset,
    Hex(u32),
}

impl FromStr for Color {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            BLACK => Ok(Color::Black),
            DARK_BLUE => Ok(Color::DarkBlue),
            DARK_GREEN => Ok(Color::DarkGreen),
            DARK_AQUA => Ok(Color::DarkAqua),
            DARK_RED => Ok(Color::DarkRed),
            DARK_PURPLE => Ok(Color::DarkPurple),
            GOLD => Ok(Color::Gold),
            GRAY => Ok(Color::Gray),
            DARK_GRAY => Ok(Color::DarkGray),
            BLUE => Ok(Color::Blue),
            GREEN => Ok(Color::Green),
            AQUA => Ok(Color::Aqua),
            RED => Ok(Color::Red),
            LIGHT_PURPLE => Ok(Color::LightPurple),
            YELLOW => Ok(Color::Yellow),
            WHITE => Ok(Color::White),
            RESET => Ok(Color::Reset),
            other => {
                if other.len() == 7 && other.as_bytes()[0] == b'#' {
                    let mut hex = 0;
                    for (ind, byte) in other[1..].bytes().rev().enumerate() {
                        hex |= (match byte {
                            b'0'..=b'9' => byte - b'0',
                            b'a'..=b'f' => byte - b'a' + 10,
                            b'A'..=b'F' => byte - b'A' + 10,
                            _ => return Err(Error::Malformed),
                        } as u32)
                            << (ind << 2);
                    }
                    Ok(Color::Hex(hex))
                } else {
                    Err(Error::Malformed)
                }
            }
        }
    }
}

impl TryFrom<&str> for Color {
    type Error = Error;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        value.parse()
    }
}

impl From<&Color> for Cow<'_, str> {
    fn from(value: &Color) -> Self {
        if let &Color::Hex(hex) = value {
            let mut buf = Vec::with_capacity(7);
            buf.push(b'#');
            for i in 2..8 {
                let value = ((hex >> (i << 2)) & 15) as u8;
                buf.push(value + (if value < 10 { b'0' } else { b'A' - 10 }));
            }
            Cow::Owned(unsafe { String::from_utf8_unchecked(buf) })
        } else {
            Cow::Borrowed(match value {
                Color::Black => BLACK,
                Color::DarkBlue => DARK_BLUE,
                Color::DarkGreen => DARK_GREEN,
                Color::DarkAqua => DARK_AQUA,
                Color::DarkRed => DARK_RED,
                Color::DarkPurple => DARK_PURPLE,
                Color::Gold => GOLD,
                Color::Gray => GRAY,
                Color::DarkGray => DARK_GRAY,
                Color::Blue => BLUE,
                Color::Green => GREEN,
                Color::Aqua => AQUA,
                Color::Red => RED,
                Color::LightPurple => LIGHT_PURPLE,
                Color::Yellow => YELLOW,
                Color::White => WHITE,
                Color::Reset => RESET,
                _ => unreachable!(),
            })
        }
    }
}

impl Display for Color {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let repr: Cow<'_, str> = self.into();
        f.write_str(&repr)
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "snake_case", tag = "action", content = "value")]
pub enum ChatClickEvent<'a> {
    OpenUrl(Cow<'a, str>),
    RunCommand(Cow<'a, str>),
    TwitchUserInfo(Cow<'a, str>),
    SuggestCommand(Cow<'a, str>),
    ChangePage(usize),
    CopyToClipboard(Cow<'a, str>)
}

impl<'a> ChatClickEvent<'a> {
    pub fn into_owned(self) -> ChatClickEvent<'static> {
        match self {
            ChatClickEvent::OpenUrl(url) => ChatClickEvent::OpenUrl(Cow::Owned(url.into_owned())),
            ChatClickEvent::RunCommand(command) => ChatClickEvent::RunCommand(Cow::Owned(command.into_owned())),
            ChatClickEvent::TwitchUserInfo(username) => ChatClickEvent::TwitchUserInfo(Cow::Owned(username.into_owned())),
            ChatClickEvent::SuggestCommand(command) => ChatClickEvent::SuggestCommand(Cow::Owned(command.into_owned())),
            ChatClickEvent::CopyToClipboard(text) => ChatClickEvent::CopyToClipboard(Cow::Owned(text.into_owned())),
            ChatClickEvent::ChangePage(page) => ChatClickEvent::ChangePage(page)
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "snake_case", tag = "action", content = "value")]
pub enum ChatHoverEvent<'a> {
    ShowText(Box<Chat<'a>>),
    // TODO: Serialize and deserialize JSON-NBT
    // JSON-NBT (a.k.a. SNBT)
    ShowItem(Cow<'a, str>),
    // JSON-NBT (a.k.a. SNBT)
    ShowEntity(Cow<'a, str>),
    ShowAchievement(Cow<'a, str>),
}

impl<'a> ChatHoverEvent<'a> {
    pub fn into_owned(self) -> ChatHoverEvent<'static> {
        match self {
            ChatHoverEvent::ShowText(text) => ChatHoverEvent::ShowText(Box::new(text.into_owned())),
            ChatHoverEvent::ShowItem(item) => ChatHoverEvent::ShowItem(Cow::Owned(item.into_owned())),
            ChatHoverEvent::ShowEntity(entity) => ChatHoverEvent::ShowEntity(Cow::Owned(entity.into_owned())),
            ChatHoverEvent::ShowAchievement(achievement) => ChatHoverEvent::ShowAchievement(Cow::Owned(achievement.into_owned()))
        }
    }
}

#[skip_serializing_none]
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ChatScore<'a> {
    name: Cow<'a, str>,
    objective: Cow<'a, str>,
    value: Option<Cow<'a, str>>,
}

impl<'a> ChatScore<'a> {
    pub fn into_owned(self) -> ChatScore<'static> {
        ChatScore {
            name: Cow::Owned(self.name.into_owned()),
            objective: Cow::Owned(self.objective.into_owned()),
            value: self.value.map(|val| Cow::Owned(val.into_owned()))
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(untagged)]
pub enum ChatValue<'a> {
    Text {
        text: Cow<'a, str>,
    },
    Translate {
        translate: Cow<'a, str>,
        with: Vec<Chat<'a>>,
    },
    Score {
        score: ChatScore<'a>,
    },
    Keybind {
        keybind: Cow<'a, str>,
    },
    Selector {
        selector: Cow<'a, str>,
    },
}

impl<'a> ChatValue<'a> {
    pub fn into_owned(self) -> ChatValue<'static> {
        match self {
            ChatValue::Text { text } => ChatValue::Text { text: Cow::Owned(text.into_owned()) },
            ChatValue::Translate { translate, with } => ChatValue::Translate { translate: Cow::Owned(translate.into_owned()), with: with.into_iter().map(Chat::into_owned).collect() },
            ChatValue::Score { score } => ChatValue::Score { score: score.into_owned() },
            ChatValue::Keybind { keybind } => ChatValue::Keybind { keybind: Cow::Owned(keybind.into_owned()) },
            ChatValue::Selector { selector } => ChatValue::Selector { selector: Cow::Owned(selector.into_owned()) },
        }
    }
}

#[serde_with::skip_serializing_none]
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ChatObject<'a> {
    bold: Option<bool>,
    italic: Option<bool>,
    underlined: Option<bool>,
    strikethrough: Option<bool>,
    obfuscated: Option<bool>,
    color: Option<Color>,
    insertion: Option<Cow<'a, str>>,
    click_event: Option<ChatClickEvent<'a>>,
    hover_event: Option<ChatHoverEvent<'a>>,
    extra: Option<Vec<Chat<'a>>>,
    #[serde(flatten)]
    value: ChatValue<'a>,
}

impl<'a> ChatObject<'a> {
    pub fn into_owned(self) -> ChatObject<'static> {
        ChatObject {
            insertion: self.insertion.map(|val| Cow::Owned(val.into_owned())),
            click_event: self.click_event.map(ChatClickEvent::into_owned),
            hover_event: self.hover_event.map(ChatHoverEvent::into_owned),
            extra: self.extra.map(|extra| extra.into_iter().map(Chat::into_owned).collect()),
            value: self.value.into_owned(),

            // tried ..self, borrowck complained
            bold: self.bold,
            italic: self.italic,
            underlined: self.underlined,
            strikethrough: self.strikethrough,
            obfuscated: self.obfuscated,
            color: self.color
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(untagged)]
pub enum Chat<'a> {
    Raw(Cow<'a, str>),
    Array(Vec<Chat<'a>>),
    Object(ChatObject<'a>),
}

impl<'a> Chat<'a> {
    // Note: version fix cannot be undone, i.e. roundtrip is lossy
    fn fix_version(&mut self, version: ProtocolVersion) {
        match self {
            Chat::Object(object) => {
                if let Some(extra) = &mut object.extra {
                    for chat in extra {
                        chat.fix_version(version);
                    }
                }
                if let ChatValue::Translate { with, .. } = &mut object.value {
                    for chat in with {
                        chat.fix_version(version);
                    }
                }
                if let Some(ChatHoverEvent::ShowText(chat)) = &mut object.hover_event {
                    chat.fix_version(version);
                }
                if version >= ProtocolVersion::V1_12 {
                    if let Some(ChatHoverEvent::ShowAchievement(achievement)) =
                        object.hover_event.take()
                    {
                        object.hover_event =
                            Some(ChatHoverEvent::ShowText(Box::new(Chat::Raw(achievement))));
                    }
                }
                if version > ProtocolVersion::V1_8_9 {
                    if let Some(ChatClickEvent::TwitchUserInfo(username)) = &object.click_event {
                        const TWITCH_URL_PREFIX: &str = "https://twitch.tv/";
                        let mut url =
                            String::with_capacity(TWITCH_URL_PREFIX.len() + username.len());
                        url.push_str(TWITCH_URL_PREFIX);
                        url.push_str(username);
                        object.click_event = Some(ChatClickEvent::OpenUrl(Cow::Owned(url)));
                    }
                }
                if version < ProtocolVersion::V1_16 {
                    if let Some(Color::Hex(_)) = object.color {
                        object.color = Some(Color::Reset);
                    }
                }
            }
            Chat::Array(array) => {
                for chat in array {
                    chat.fix_version(version);
                }
            }
            _ => {}
        }
    }

    pub fn into_owned(self) -> Chat<'static> {
        match self {
            Chat::Raw(text) => Chat::Raw(Cow::Owned(text.into_owned())),
            Chat::Array(array) => Chat::Array(array.into_iter().map(Chat::into_owned).collect()),
            Chat::Object(object) => Chat::Object(object.into_owned())
        }
    }
}

decode_impl!(Chat<'a>, src, version, {
    serde_json::from_str(&LengthCappedString::<262144>::decode(src, version).await?.0)
        .map_err(|_| Error::Malformed)
});

encode_impl!(Chat<'a>, self, tgt, version, {
    self.fix_version(version);
    let chat: LengthCappedString<262144> = serde_json::to_string(&self)
        .map_err(|_| Error::Malformed)?
        .try_into()?;
    chat.encode(tgt, version).await
});

#[derive(Copy, Clone, Debug, PartialEq, Eq, SerializeDisplay, DeserializeFromStr)]
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
                (num << 26 >> 52) as i32,
                (num << 38 >> 38) as i32,
            )
        };
        Position { x, y, z }
    })
});

encode_impl!(Position, self, tgt, version, {
    if self.x > 33554431
        || self.x < -33554432
        || self.y > 2047
        || self.y < -2048
        || self.z > 33554431
        || self.z < -33554431
    {
        Err(Error::Malformed)
    } else {
        let num = if version >= ProtocolVersion::V1_14_4 {
            ((self.x as u64) << 38) | ((self.z as u64) << 12) | (self.y as u64)
        } else {
            ((self.x as u64) << 38) | ((self.y as u64) << 26) | (self.z as u64)
        };
        tgt.write_u64(num).await.map_err(handle_io_err)
    }
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
