#![feature(type_alias_impl_trait)]
#![feature(generic_associated_types)]

mod connection;
mod protocol;
mod web;

use futures::future::join_all;
use protocol::version::ProtocolVersion;

use connection::{
    packets::login::{Player, ServerLoginCredentials},
    Client, ServerConnection, State,
};
use protocol::error::Error as ProtocolError;
use reqwest::Client as HTTPClient;
use std::{
    borrow::Cow, cell::RefCell, collections::HashMap, env::args, error::Error, io::Cursor, rc::Rc,
};
use tokio::{
    io::{copy, AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    try_join,
    task
};
use web::yggdrasil::{Authentication, OnlineMode, UserInfo};

use crate::{
    protocol::types::{Chat, Decode, Encode, LengthCappedString, VarInt, UUID},
    web::{hypixel::Hypixel, mojang::Mojang},
};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn Error>> {
    task::LocalSet::new().run_until(async move {
        let listener = TcpListener::bind("localhost:25565").await?;
        let mut args = std::env::args();
        let username = args.nth(1).unwrap();
        let password = args.nth(0).unwrap();
        let api_key = args.nth(0).unwrap();
        loop {
            let conn = listener.accept().await?.0;
            let username = username.clone();
            let password = password.clone();
            let api_key = api_key.clone();
            task::spawn_local(async move {
                if let Err(err) = async {
                    let mut conn = ServerConnection::new(conn).await;
                    conn.accept_handshake().await?;
                    let mut client = Client::connect("mc.hypixel.net", ProtocolVersion::V1_8_9).await?;
                    client.handshake(conn.state).await?;
                    if conn.state == State::Status {
                        let packet = conn.inbound.next_packet().await?;
                        if packet.id != 0 || packet.len != 0 {
                            Err(ProtocolError::Malformed)?;
                        }
                        client.outbound.create_packet(0, Some(0)).await?;
                        loop {
                            let mut packet = client.inbound.next_packet().await?;
                            let mut out_packet = conn
                                .outbound
                                .create_packet(packet.id, Some(packet.len))
                                .await?;
                            copy(&mut packet.content, &mut out_packet).await?;
                            packet.content.finished()?;
                            out_packet.shutdown().await?;
                        }
                    } else {
                        let web_client = HTTPClient::new();
                        let mut auth =
                            Authentication::new(Some("my_client_name"), None, Some(web_client.clone()));
                        let UserInfo { name, id } = auth
                            .authenticate(&username, &password)
                            .await?
                            .user_info;
                        conn.accept_login(|nm| async {
                            Ok(ServerLoginCredentials::OfflineMode(Player {
                                username: Cow::Borrowed(&name),
                                uuid: id,
                            }))
                        })
                        .await?;
                        client
                            .login(
                                Some(web_client.clone()),
                                OnlineMode::new(
                                    UserInfo {
                                        name: Cow::Owned(name.into_owned()),
                                        id,
                                    },
                                    auth,
                                ),
                                Client::NO_LOGIN_PLUGIN_HANDLER,
                            )
                            .await?;
        
                        let Client {
                            inbound,
                            outbound,
                            version,
                            ..
                        } = &mut client;
                        let version = *version;
                        let send_to_client = Rc::new(RefCell::new(Vec::new()));
                        let all_local_players = Rc::new(RefCell::new(HashMap::<UUID, String>::new()));
                        let ServerConnection {
                            inbound: server_inbound,
                            outbound: server_outbound,
                            version: server_version,
                            ..
                        } = &mut conn;
                        let server_version = *server_version;
                        let client = web_client.clone();
                        let mojang = Mojang::new(None, Some(client.clone()));
                        let hypixel = Hypixel::new(&api_key, Some(client.clone()));
                        try_join!(
                            async {
                                loop {
                                    let mut packet = server_inbound.next_packet().await?;
                                    match packet.id {
                                        1 => {
                                            let msg = LengthCappedString::<256>::decode(
                                                &mut packet.content,
                                                server_version,
                                            )
                                            .await?;
                                            packet.content.finished()?;
                                            if msg.0.starts_with("/stats ") {
                                                let uname = &msg.0[7..];
                                                let players = if uname == "*" {
                                                    all_local_players
                                                        .borrow()
                                                        .iter()
                                                        .map(|(_, v)| Cow::Owned(v.into()))
                                                        .collect()
                                                } else {
                                                    Vec::from([Cow::Borrowed(uname)])
                                                };
                                                let good_players =
                                                    join_all(players.into_iter().map(|player| {
                                                        let mojang = &mojang;
                                                        let hypixel = &hypixel;
                                                        let send_to_client = send_to_client.clone();
                                                        async move {
                                                            let mut out = None;
                                                            let mut result: Cow<'_, str>;
                                                            // todo: optimize
                                                            if let Ok(uuid) = mojang.get_uuid(&player).await
                                                            {
                                                                let player_info =
                                                                    hypixel.info(uuid).await?;
                                                                result = "Unknown".into();
                                                                if let Some(bw_stats) =
                                                                    player_info.stats.bedwars
                                                                {
                                                                    let fkdr = bw_stats
                                                                        .final_kills
                                                                        .map_or(0.0, |v| v as f64)
                                                                        / bw_stats
                                                                            .final_deaths
                                                                            .map_or(1.0, |v| v as f64);
                                                                    if fkdr > 2.0 {
                                                                        out = Some((
                                                                            fkdr,
                                                                            player.to_string(),
                                                                        ));
                                                                    }
                                                                    result = format!("{:.2}", fkdr).into();
                                                                }
                                                            } else {
                                                                result = "Nicked".into();
                                                                out = Some((-1.0, player.to_string()));
                                                            }
                                                            send_to_client.borrow_mut().push(Chat::Raw(
                                                                [&player, ": ", &result].concat().into(),
                                                            ));
                                                            // Suppress output
                                                            Ok::<Option<(f64, String)>, Box<dyn Error>>(out)
                                                        }
                                                    }))
                                                    .await;
                                                if uname == "*" {
                                                    for good_player in good_players {
                                                        if let Some((fkdr, name)) = good_player? {
                                                            let mut out_packet =
                                                                outbound.create_packet(1, None).await?;
                                                            let warning = if fkdr < 0.0 {
                                                                format!("/pc {} is nicked", name)
                                                            } else {
                                                                format!(
                                                                    "/pc {} has {:.2} FKDR",
                                                                    name, fkdr
                                                                )
                                                            };
                                                            LengthCappedString::<256>(warning.into())
                                                                .encode(&mut out_packet, version)
                                                                .await?;
                                                            out_packet.shutdown().await?;
                                                        }
                                                    }
                                                }
                                            } else {
                                                let mut out_packet = outbound
                                                    .create_packet(packet.id, Some(packet.len))
                                                    .await?;
                                                msg.encode(&mut out_packet, version).await?;
                                                out_packet.shutdown().await?;
                                            }
                                        }
                                        _ => {
                                            let mut out_packet =
                                                outbound.create_packet(packet.id, Some(packet.len)).await?;
                                            copy(&mut packet.content, &mut out_packet).await?;
                                            packet.content.finished()?;
                                            out_packet.shutdown().await?;
                                        }
                                    }
                                }
                                Ok::<(), Box<dyn Error>>(())
                            },
                            async {
                                loop {
                                    while let Some(out_chat) = send_to_client.borrow_mut().pop() {
                                        let mut out_packet = server_outbound.create_packet(2, None).await?;
                                        out_chat.encode(&mut out_packet, server_version).await?;
                                        1u8.encode(&mut out_packet, server_version).await?;
                                        out_packet.shutdown().await?;
                                    }
                                    let mut packet = inbound.next_packet().await?;
                                    match packet.id {
                                        0x38 => {
                                            let mut vec = Vec::with_capacity(packet.len);
                                            packet.content.read_to_end(&mut vec).await?;
                                            packet.content.finished()?;
                                            let mut content = Cursor::new(&vec);
                                            let action = VarInt::decode(&mut content, version).await?.0;
                                            let num_players =
                                                VarInt::decode(&mut content, version).await?.0;
                                            for _ in 0..num_players {
                                                let uuid = UUID::decode(&mut content, version).await?;
                                                match action {
                                                    0 => {
                                                        let name = LengthCappedString::<16>::decode(
                                                            &mut content,
                                                            version,
                                                        )
                                                        .await?
                                                        .0;
                                                        all_local_players
                                                            .borrow_mut()
                                                            .insert(uuid, name.into());
                                                        for _ in 0..VarInt::decode(&mut content, version)
                                                            .await?
                                                            .0
                                                        {
                                                            LengthCappedString::<32767>::decode(
                                                                &mut content,
                                                                version,
                                                            )
                                                            .await?;
                                                            LengthCappedString::<32767>::decode(
                                                                &mut content,
                                                                version,
                                                            )
                                                            .await?;
                                                            if bool::decode(&mut content, version).await? {
                                                                LengthCappedString::<32767>::decode(
                                                                    &mut content,
                                                                    version,
                                                                )
                                                                .await?;
                                                            }
                                                        }
                                                        VarInt::decode(&mut content, version).await?;
                                                        VarInt::decode(&mut content, version).await?;
                                                        if bool::decode(&mut content, version).await? {
                                                            Chat::decode(&mut content, version).await?;
                                                        }
                                                    }
                                                    4 => {
                                                        all_local_players.borrow_mut().remove(&uuid);
                                                    }
                                                    _ => {}
                                                }
                                            }
                                            content.set_position(0);
                                            let mut out_packet = server_outbound
                                                .create_packet(packet.id, Some(packet.len))
                                                .await?;
                                            copy(&mut content, &mut out_packet).await?;
                                            packet.content.finished()?;
                                            out_packet.shutdown().await?;
                                        }
                                        _ => {
                                            let mut out_packet = server_outbound
                                                .create_packet(packet.id, Some(packet.len))
                                                .await?;
                                            copy(&mut packet.content, &mut out_packet).await?;
                                            packet.content.finished()?;
                                            out_packet.shutdown().await?;
                                        }
                                    }
                                }
                                Ok::<(), Box<dyn Error>>(())
                            }
                        )?;
                    }
                    Ok::<(), Box<dyn Error>>(())
                }
                .await
                {
                    println!("Connection closed: {}", err);
                }
            });
        }
        Ok(())
    }).await
}
