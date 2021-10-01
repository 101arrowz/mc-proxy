#![feature(type_alias_impl_trait)]
#![feature(generic_associated_types)]
#![feature(trait_alias)]

mod connection;
mod protocol;
mod web;

use protocol::version::ProtocolVersion;

use connection::{Client, ServerConnection, State, packets::{login::{ServerLoginCredentials, Player}}};
use tokio::net::TcpListener;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind("localhost:25565").await?;
    let mut conn = ServerConnection::new(listener.accept().await?.0).await;
    conn.accept_handshake().await?;
    let web_client = reqwest::Client::new();
    let mut auth = web::yggdrasil::Authentication::new(
        Some("my_client_name"),
        None,
        Some(web_client.clone()),
    );
    let mut args = std::env::args();
    let web::yggdrasil::UserInfo { name, id } = auth
        .authenticate(&args.nth(1).unwrap(), &args.nth(0).unwrap())
        .await?
        .user_info;
    {
        let mut client = Client::connect("mc.hypixel.net", ProtocolVersion::V1_8_9).await?;
        client.handshake(State::Login).await?;
        conn.accept_login(|_| async {
            Ok(ServerLoginCredentials::OfflineMode(Player {
                username: std::borrow::Cow::Borrowed(&name),
                uuid: id
            }))
        }).await?;
        client
            .login(
                Some(web_client),
                web::yggdrasil::OnlineMode::new(
                    web::yggdrasil::UserInfo {
                        name: std::borrow::Cow::Owned(name.into_owned()),
                        id,
                    },
                    auth,
                ),
                Client::NO_LOGIN_PLUGIN_HANDLER,
            )
            .await?;
        conn.play_proxy(&mut client).await?
    }
    Ok(())
}
