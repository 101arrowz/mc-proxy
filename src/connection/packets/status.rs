use std::borrow::Cow;

use crate::connection::{error::Error, Client, State};
use crate::protocol::types::Chat;
use crate::protocol::{
    error::Error as ProtocolError,
    types::{Decode, Encode, LengthCappedString, UUID},
};

use serde_with::skip_serializing_none;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Version<'a> {
    name: Cow<'a, str>,
    protocol: i32,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SamplePlayer<'a> {
    name: Cow<'a, str>,
    id: UUID,
}

#[skip_serializing_none]
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Players<'a> {
    max: usize,
    online: usize,
    sample: Option<Vec<SamplePlayer<'a>>>,
}

#[skip_serializing_none]
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Status<'a> {
    version: Version<'a>,
    players: Players<'a>,
    description: Chat<'a>,
    favicon: Option<Cow<'a, str>>,
}

impl Client {
    pub async fn status<'a>(&'a mut self) -> Result<Status<'a>, Error> {
        if self.state == State::Status {
            self.outbound.create_packet(0, Some(0)).await?;
            loop {
                let mut packet = self.inbound.next_packet().await?;
                // Compression not enabled, so shutdown unnecessary
                match packet.id {
                    0 => {
                        let status_str =
                            &LengthCappedString::<32767>::decode(&mut packet.content, self.version)
                                .await?
                                .0;
                        match serde_json::from_str(status_str) {
                            Ok(status) => break Ok(status),
                            Err(_) => Err(ProtocolError::Malformed)?,
                        }
                    }
                    1 => {
                        i64::decode(&mut packet.content, self.version)
                            .await?
                            .encode(
                                &mut self.outbound.create_packet(1, Some(8)).await?,
                                self.version,
                            )
                            .await?;
                    }
                    _ => Err(ProtocolError::Malformed)?,
                }
                packet.content.finished()?;
            }
        } else {
            Err(Error::InvalidState)
        }
    }
}
