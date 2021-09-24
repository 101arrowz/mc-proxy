use crate::connection::{error::Error, Client, State};
use crate::protocol::types::Chat;
use crate::protocol::{
    error::Error as ProtocolError,
    types::{Decode, Encode, LengthCappedString, UUID},
};

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Version {
    name: String,
    protocol: i32,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SamplePlayer {
    name: String,
    id: UUID,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Players {
    max: usize,
    online: usize,
    sample: Option<Vec<SamplePlayer>>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Status {
    version: Version,
    players: Players,
    description: Chat,
    favicon: Option<String>,
}

impl Client {
    pub async fn status(&mut self) -> Result<Status, Error> {
        if self.state == State::Status {
            self.outbound.create_packet(0, Some(0)).await?;
            loop {
                let mut packet = self.inbound.next_packet().await?;
                match packet.id {
                    0 => {
                        let status_str = &LengthCappedString::<32767>::decode(&mut packet.content, self.version)
                            .await?
                            .0;
                        match serde_json::from_str(
                            status_str,
                        ) {
                            Ok(status) => break Ok(status),
                            Err(_) => Err(ProtocolError::Malformed)?
                        }
                    }
                    1 => {
                        i64::decode(&mut packet.content, self.version)
                            .await?
                            .encode(&mut self.outbound.create_packet(1, Some(8)).await?, self.version)
                            .await?;
                    }
                    _ => Err(ProtocolError::Malformed)?,
                }
            }
        } else {
            Err(Error::InvalidState)
        }
    }
}
