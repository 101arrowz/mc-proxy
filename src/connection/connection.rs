use std::{convert::TryInto, io::Cursor};
use tokio::{
    io::AsyncWriteExt,
    net::{
        tcp::{OwnedReadHalf, OwnedWriteHalf},
        TcpStream,
    },
};
use trust_dns_resolver::{
    config::{ResolverConfig, ResolverOpts},
    AsyncResolver,
};

use crate::protocol::{
    types::{Encode, LengthCappedString, VarInt},
    version::ProtocolVersion,
};

use super::{
    codec::{InboundConnection, OutboundConnection},
    error::Error,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum State {
    Handshaking = 0,
    Status = 1,
    Login = 2,
    Play = 3,
}

pub struct Client {
    outbound: OutboundConnection<OwnedWriteHalf>,
    inbound: InboundConnection<OwnedReadHalf>,
    host: String,
    port: u16,
    state: State,
    version: ProtocolVersion,
}

impl Client {
    pub async fn connect(target: &str, version: ProtocolVersion) -> Result<Client, Error> {
        let resolver =
            AsyncResolver::tokio(ResolverConfig::default(), ResolverOpts::default()).unwrap();
        let mut target = target.split(':');
        let domain = target.next().unwrap().to_string();
        let port = target.next().map_or(Ok(None), |port| match port.parse() {
            Err(_) => Err(Error::InvalidTarget),
            Ok(port) => Ok(Some(port)),
        })?;
        let (host, port) = if let Ok(srv) = resolver
            .srv_lookup(["_minecraft._tcp.", domain.as_str()].concat())
            .await
        {
            let record = srv.iter().next().unwrap();
            let host = record.target().to_string();
            (host, port.unwrap_or(record.port()))
        } else {
            (domain, port.unwrap_or(25565))
        };
        let ip_addr = match resolver.lookup_ip(host.as_str()).await {
            Ok(lookup) => lookup.iter().next().unwrap(),
            Err(_) => return Err(Error::InvalidTarget),
        };
        let (read_half, write_half) = TcpStream::connect((ip_addr, port)).await?.into_split();
        Ok(Client {
            outbound: OutboundConnection::new(write_half, version),
            inbound: InboundConnection::new(read_half, version),
            state: State::Handshaking,
            host,
            port,
            version,
        })
    }

    pub async fn handshake(&mut self, next_state: State) -> Result<(), Error> {
        if next_state != State::Status && next_state != State::Login {
            Err(Error::InvalidState)
        } else {
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
            Ok(())
        }
    }
}
