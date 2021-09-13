#![feature(type_alias_impl_trait)]
#![feature(generic_associated_types)]
#![feature(associated_type_defaults)]
#![feature(concat_idents)]

use tokio::io;
use tokio::net::{TcpListener, TcpStream};
use trust_dns_resolver::config::{ResolverConfig, ResolverOpts};
use trust_dns_resolver::AsyncResolver;

mod protocol;
mod connection;

use protocol::{types::{VarInt, MCDecode, MCEncode}, version::ProtocolVersion};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let target = "lunar.gg";
    let resolver = AsyncResolver::tokio(ResolverConfig::default(), ResolverOpts::default())?;
    let (remote_host, remote_port) = if let Ok(srv) = resolver
        .srv_lookup(["_minecraft._tcp.", target].concat())
        .await
    {
        let record = srv.iter().next().unwrap();
        let host = record.target().to_string();
        (host, record.port())
    } else {
        (target.to_string(), 25565)
    };
    let ip_addr = resolver
        .lookup_ip(remote_host.to_string())
        .await?
        .iter()
        .next()
        .unwrap();
    let local_server = TcpListener::bind("localhost:25565").await?;
    loop {
        let (mut client, addr) = local_server.accept().await?;
        let remote_host = remote_host.clone();
        tokio::spawn(async move {
            let (mut client_out, mut client_in) = client.split();
            if let Ok(mut server) = TcpStream::connect((ip_addr, remote_port)).await {
                let (mut server_out, mut server_in) = server.split();
                let err = tokio::join!(async {
                    let value: i32 = VarInt::decode(&mut client_out, ProtocolVersion::V1_8).await?.into();
                    io::copy(&mut client_out, &mut server_in).await?;
                    Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
                });
            }
        });
    }
}
