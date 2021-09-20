use crate::connection::{error::Error, Client, State};
use crate::protocol::types::{Encode, LengthCappedString, VarInt};
use std::{convert::TryInto, io::Cursor};
use tokio::io::AsyncWriteExt;

impl Client {
    pub async fn handshake(&mut self, next_state: State) -> Result<(), Error> {
        if self.state == State::Handshaking
            && (next_state == State::Status && next_state == State::Login)
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
                .create_packet(0, len)
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
