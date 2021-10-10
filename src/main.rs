#![feature(type_alias_impl_trait)]
#![feature(generic_associated_types)]
#![feature(iter_intersperse)]

mod connection;
mod protocol;
mod web;

use futures::future::join_all;

use bimap::BiHashMap;
use connection::{
    packets::login::{Player, ServerLoginCredentials},
    Client, ServerConnection, State,
};
use protocol::error::Error as ProtocolError;
use reqwest::Client as HTTPClient;
use std::{borrow::Cow, cell::RefCell, env::args, error::Error, io::Cursor};
use tokio::{
    io::{copy, AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    task, try_join,
};
use unicase::Ascii;
use web::yggdrasil::{Authentication, OnlineMode, UserInfo};

use crate::{
    protocol::types::{
        Chat, ChatObject, ChatValue, Color, Decode, Encode, LengthCappedString, VarInt, UUID,
    },
    web::{hypixel::Hypixel, mojang::Mojang},
};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn Error>> {
    task::LocalSet::new().run_until(async move {
        let listener = TcpListener::bind("localhost:25565").await?;
        let mut args = args();
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
                    let mut client = Client::connect("mc.hypixel.net", conn.version).await?;
                    client.handshake(conn.state).await?;
                    if conn.state == State::Status {
                        let packet = conn.inbound.next_packet().await?;
                        if packet.id != 0 || packet.len != 0 {
                            return return Err(ProtocolError::Malformed.into());
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
                        conn.accept_login(|_| async {
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
                        let send_to_client = RefCell::new(Vec::new());
                        let all_local_players = RefCell::new(BiHashMap::<UUID, Ascii<Cow<'_, str>>>::new());
                        let ServerConnection {
                            inbound: server_inbound,
                            outbound: server_outbound,
                            version: server_version,
                            ..
                        } = &mut conn;
                        let server_version = *server_version;
                        let client = web_client.clone();
                        let mojang = Mojang::new(None, Some(client.clone()));
                        let hypixel = Hypixel::new(&api_key, Some(client));
                        try_join!(
                            async {
                                loop {
                                    let mut packet = server_inbound.next_packet().await?;
                                    match packet.id {
                                        1 => {
                                            let orig_msg = LengthCappedString::<256>::decode(
                                                &mut packet.content,
                                                server_version,
                                            )
                                            .await?;
                                            let msg = &orig_msg.0;
                                            packet.content.finished()?;
                                            if msg.starts_with("/stats ") {
                                                let unames = &msg[7..];
                                                let players = if unames == "*" {
                                                    all_local_players
                                                        .borrow()
                                                        .iter()
                                                        .map(|(&uuid, v)| (Some(uuid), Cow::Owned(v.as_ref().into())))
                                                        .collect::<Vec<_>>()
                                                } else {
                                                    unames.split(' ').map(|uname| (all_local_players.borrow().get_by_right(&Ascii::new(uname.into())).copied(), Cow::Borrowed(uname))).collect()
                                                };
                                                let good_players =
                                                    join_all(players.into_iter().map(|player| {
                                                        let mojang = &mojang;
                                                        let hypixel = &hypixel;
                                                        let send_to_client = &send_to_client;
                                                        let mut uuid = player.0;
                                                        let mut player = player.1.into_owned();
                                                        async move {
                                                            let mut out = Vec::<Cow<'_, str>>::new();
                                                            let mut display = Vec::<Chat<'_>>::new();
                                                            let uuid_lookup = uuid.is_none();
                                                            if uuid_lookup {
                                                                uuid = mojang.get_uuid(&player).await.ok().map(|(uuid, name)| {
                                                                    player = name;
                                                                    uuid
                                                                });
                                                            }
                                                            let mut player_info = None;
                                                            let mut nicked = true;
                                                            if let Some(uuid) = uuid {
                                                                if let Some(info) = hypixel.info(uuid).await? {
                                                                    nicked = false;
                                                                    let stats = &info.stats;
                                                                    if let Some(bw_stats) = &stats.bedwars {
                                                                        let fkdr = bw_stats
                                                                            .final_kills
                                                                            .map_or(0.0, |v| v as f64)
                                                                            / bw_stats
                                                                                .final_deaths
                                                                                .map_or(1.0, |v| v as f64);
                                                                        if fkdr > 2.0 {
                                                                            out.push(format!(
                                                                                "has {:.2} FKDR",
                                                                                fkdr,
                                                                            ).into());
                                                                        }
                                                                        display.push(Chat::Raw(format!("{:.2} FKDR", fkdr).into()));
                                                                    }
                                                                    player_info = Some(info);
                                                                } else if uuid_lookup || mojang.get_uuid(&player).await.is_ok() {
                                                                    nicked = false;
                                                                }
                                                            }
                                                            if nicked {
                                                                out.push("is nicked".into());
                                                            }
                                                            let out = if out.is_empty() {
                                                                None
                                                            } else {
                                                                Some(format!("{} {}", &player, out.join(", ")))
                                                            };
                                                            send_to_client.borrow_mut().push(
                                                                if let Some(ref player_info) = player_info {
                                                                    Chat::Array(vec![player_info.into(), Chat::Object(ChatObject {
                                                                        color: Some(Color::Reset),
                                                                        value: ChatValue::Text { text: ": ".into() },
                                                                        extra: Some(display.into_iter().intersperse(Chat::Raw(", ".into())).collect()),
                                                                        ..Default::default()
                                                                    })])
                                                                } else {
                                                                    Chat::Object(ChatObject {
                                                                        color: Some(if nicked { Color::DarkRed } else { Color::Gray }),
                                                                        value: ChatValue::Text { text: (if nicked { "[NICKED] " } else { "" }).into()  },
                                                                        extra: Some(vec![
                                                                            Chat::Raw(player.into()),
                                                                            Chat::Raw("§r: Unknown".into()),
                                                                        ]),
                                                                        ..Default::default()
                                                                    })
                                                                }
                                                            );
                                                            Ok::<Option<String>, Box<dyn Error>>(out)
                                                        }
                                                    }))
                                                    .await;
                                                if unames == "*" {
                                                    for good_player in good_players {
                                                        if let Some(msg) = good_player? {
                                                            let mut out_packet =
                                                                outbound.create_packet(1, None).await?;
                                                            LengthCappedString::<256>(format!("/pc {}", msg).into())
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
                                                orig_msg.encode(&mut out_packet, version).await?;
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
                                #[allow(unreachable_code)]
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
                                                            .insert(uuid, Ascii::new(name));
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
                                                        all_local_players.borrow_mut().remove_by_left(&uuid);
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
                                #[allow(unreachable_code)]
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
    }).await
}
