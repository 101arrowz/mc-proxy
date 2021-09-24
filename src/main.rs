#![feature(type_alias_impl_trait)]
#![feature(generic_associated_types)]
#![feature(associated_type_defaults)]

mod connection;
mod protocol;

use protocol::version::ProtocolVersion;

use connection::{Client, State};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut client = Client::connect("mc.hypixel.net", ProtocolVersion::V1_8_9).await?;
    client.handshake(State::Status).await?;
    dbg!(client.status().await?);
    Ok(())
}
