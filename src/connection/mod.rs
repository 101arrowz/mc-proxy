pub mod codec;
mod encryption;
pub mod error;
pub mod packets;
mod util;

use std::{
    convert::Infallible,
    future::{ready, Future, Ready},
    net::SocketAddr,
};

use tokio::{
    io::AsyncWriteExt,
    net::{
        tcp::{OwnedReadHalf, OwnedWriteHalf},
        TcpListener, TcpStream,
    },
};
use trust_dns_resolver::{
    config::{ResolverConfig, ResolverOpts},
    AsyncResolver,
};

use super::protocol::version::ProtocolVersion;

use codec::{InboundConnection, OutboundConnection};
use error::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
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
}

pub struct ServerConnection {
    outbound: OutboundConnection<OwnedWriteHalf>,
    inbound: InboundConnection<OwnedReadHalf>,
    state: State,
    version: ProtocolVersion,
}

impl ServerConnection {
    pub async fn new(conn: TcpStream) -> ServerConnection {
        const INIT_VERSION: ProtocolVersion = ProtocolVersion::V1_16;
        let (read_half, write_half) = conn.into_split();
        ServerConnection {
            outbound: OutboundConnection::new(write_half, INIT_VERSION),
            inbound: InboundConnection::new(read_half, INIT_VERSION),
            state: State::Handshaking,
            version: INIT_VERSION,
        }
    }
}
