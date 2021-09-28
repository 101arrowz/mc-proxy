#![feature(type_alias_impl_trait)]
#![feature(generic_associated_types)]

mod connection;
mod protocol;
mod web;

use protocol::version::ProtocolVersion;

use connection::{Client, State};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    {
        let mut client = Client::connect("mc.hypixel.net", ProtocolVersion::V1_8_9).await?;
        client.handshake(State::Status).await?;
        dbg!(client.status().await?);
    }
    {
        let mut client = Client::connect("mc.hypixel.net", ProtocolVersion::V1_8_9).await?;
        client.handshake(State::Login).await?;
        let web_client = reqwest::Client::new();
        let mut auth = web::yggdrasil::Authentication::new(Some("my_client_name"), None, Some(web_client.clone()));
        let mut args = std::env::args();
        let user_info = auth.authenticate(&args.nth(1).unwrap(), &args.nth(0).unwrap()).await?.user_info;
        client.login(Some(web_client), web::yggdrasil::OnlineMode::new(web::yggdrasil::UserInfo {
            name: std::borrow::Cow::Owned(user_info.name.into_owned()),
            id: user_info.id
        }, auth), Client::NO_PLUGIN_HANDLER).await?;
    }
    Ok(())
}
