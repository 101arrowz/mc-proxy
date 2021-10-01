use crate::connection::{error::Error, Client, ServerConnection, State};
use crate::protocol::{
    error::Error as ProtocolError,
    types::{Decode, Encode, LengthCappedString, VarInt},
};
use std::{convert::TryInto, io::Cursor};
use tokio::io::AsyncWriteExt;

impl Client {
    pub async fn handshake(&mut self, next_state: State) -> Result<(), Error> {
        if self.state == State::Handshaking
            && (next_state == State::Status || next_state == State::Login)
        {
            // handshake packet at most 265 bytes across all versions, present and future
            let mut packet_buffer = [0; 265];
            let mut packet: Cursor<&mut [u8]> = Cursor::new(&mut packet_buffer);
            VarInt(self.version as i32)
                .encode(&mut packet, self.version)
                .await?;
            let server_address: LengthCappedString<255> = self.host.as_str().try_into()?;
            server_address.encode(&mut packet, self.version).await?;
            self.port.encode(&mut packet, self.version).await?;
            VarInt(next_state as i32)
                .encode(&mut packet, self.version)
                .await?;
            let len = packet.position() as usize;
            self.outbound
                .create_packet(0, Some(len))
                .await?
                .write_all(&packet.get_ref()[..len])
                .await?;
            self.state = next_state;
            Ok(())
        } else {
            Err(Error::InvalidState)
        }
    }
}

impl ServerConnection {
    pub async fn accept_handshake(&mut self) -> Result<(), Error> {
        if self.state == State::Handshaking {
            let mut packet = self.inbound.next_packet().await?;
            if packet.id != 0 {
                Err(ProtocolError::Malformed)?;
            }
            self.version = VarInt::decode(&mut packet.content, self.version)
                .await?
                .0
                .try_into()?;
            let _host = LengthCappedString::<256>::decode(&mut packet.content, self.version)
                .await?
                .0;
            let _port = u16::decode(&mut packet.content, self.version).await?;
            let next_state = match VarInt::decode(&mut packet.content, self.version).await?.0 {
                1 => State::Status,
                2 => State::Login,
                _ => Err(ProtocolError::Malformed)?,
            };
            packet.content.finished()?;
            self.state = next_state;
            Ok(())
        } else {
            Err(Error::InvalidState)
        }
    }
}
