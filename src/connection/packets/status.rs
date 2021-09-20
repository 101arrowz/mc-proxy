use crate::connection::{error::Error, Client, State};
use crate::protocol::{
    error::Error as ProtocolError,
    types::{Decode, Encode, LengthCappedString, UUID},
};

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug)]
pub struct Version {
    name: String,
    protocol: i32,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct SamplePlayer {
    name: String,
    id: UUID,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Players {
    max: usize,
    online: usize,
    sample: Vec<SamplePlayer>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Description {
    text: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Status {
    version: Version,
    players: Players,
    description: Description,
    favicon: Option<String>,
}

impl Client {
    pub async fn status(&mut self) -> Result<Status, Error> {
        if self.state == State::Status {
            self.outbound.create_packet(0, 0).await?;
            loop {
                let mut packet = self.inbound.next_packet().await?;
                match packet.id {
                    0 => {
                        match serde_json::from_str(
                            &LengthCappedString::<32767>::decode(&mut packet.content, self.version)
                                .await?
                                .0,
                        ) {
                            Ok(status) => break Ok(status),
                            Err(_) => Err(ProtocolError::Malformed)?,
                        }
                    }
                    1 => {
                        i64::decode(&mut packet.content, self.version)
                            .await?
                            .encode(&mut self.outbound.create_packet(1, 8).await?, self.version)
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
