use super::error::Error;
use std::convert::TryFrom;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ProtocolVersion {
    V1_8_9 = 47,
    V1_12 = 335,
    V1_14_4 = 498,
    V1_16 = 735,
}

impl TryFrom<i32> for ProtocolVersion {
    type Error = Error;

    fn try_from(val: i32) -> Result<Self, Self::Error> {
        Ok(match val {
            0..=334 => ProtocolVersion::V1_8_9,
            335..=497 => ProtocolVersion::V1_12,
            498..=734 => ProtocolVersion::V1_14_4,
            735.. => ProtocolVersion::V1_16,
            _ => return Err(Error::Malformed),
        })
    }
}
