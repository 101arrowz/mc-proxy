#![feature(type_alias_impl_trait)]
#![feature(generic_associated_types)]
#![feature(poll_ready)]
#![feature(iter_intersperse)]
#![allow(clippy::upper_case_acronyms)]

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
use std::{borrow::Cow, error::Error, io::Cursor, sync::Mutex, collections::HashMap};
use tokio::{
    io::{copy, AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    try_join,
};
use unicase::Ascii;
use web::microsoft;
use web::yggdrasil;

use crate::{
    protocol::types::{
        Chat, ChatObject, ChatValue, Color, Decode, Encode, LengthCappedString, VarInt, UUID,
    },
    web::{hypixel::Hypixel, mojang::Mojang},
};

#[derive(Debug, Clone)]
pub enum StartConfig {
    Yggdrasil { username: String, password: String },
    Microsoft { access_token: String },
}

#[derive(Debug, Clone)]
enum AuthConfig<'a> {
    Yggdrasil(yggdrasil::Authentication<'a>, yggdrasil::UserInfo<'a>),
    Microsoft(microsoft::Authentication<'a>, microsoft::UserInfo<'a>),
}

const CLIENT_NAME: &str = "mc-proxy";

pub async fn start(
    config: StartConfig,
    api_key: String,
) -> Result<(), Box<dyn Error + Send + Sync + 'static>> {
    let listener = TcpListener::bind("localhost:25565").await?;
    let web_client = HTTPClient::new();
    let mut auth_config: AuthConfig<'_>;
    let config = dbg!(config);
    match config {
        StartConfig::Yggdrasil { username, password } => {
            let mut auth =
                yggdrasil::Authentication::new(Some(CLIENT_NAME), None, Some(web_client.clone()));
            let info = auth.authenticate(&username, &password).await?.user_info;
            auth_config = AuthConfig::Yggdrasil(auth, info);
        }
        StartConfig::Microsoft { access_token } => {
            let mut auth = microsoft::Authentication::new(
                Cow::Owned(access_token),
                None,
                Some(web_client.clone()),
            );
            let info = auth.get_info().await?;
            auth_config = AuthConfig::Microsoft(auth, info);
        }
    };
    auth_config = dbg!(auth_config);
    loop {
        let conn = listener.accept().await?.0;
        let api_key = api_key.clone();
        let web_client = web_client.clone();
        let auth_config = auth_config.clone();
        tokio::spawn(async move {
            if let Err(err) = async {
                let mut conn = ServerConnection::new(conn).await;
                conn.accept_handshake().await?;
                let mut client = Client::connect("mc.hypixel.net", conn.version).await?;
                client.handshake(conn.state).await?;
                if conn.state == State::Status {
                    let packet = conn.inbound.next_packet().await?;
                    if packet.id != 0 || packet.len != 0 {
                        return Err(ProtocolError::Malformed.into());
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
                    let (name, id) = match auth_config {
                        AuthConfig::Yggdrasil(_, ref info) => (info.name.as_ref(), info.id),
                        AuthConfig::Microsoft(_, ref info) => (info.name.as_ref(), info.id),
                    };
                    conn.accept_login(|_| async {
                        Ok(ServerLoginCredentials::OfflineMode(Player {
                            username: Cow::Borrowed(&name),
                            uuid: id,
                        }))
                    })
                    .await?;
                    match auth_config {
                        AuthConfig::Yggdrasil(auth, _) => {
                            client
                                .login(
                                    Some(web_client.clone()),
                                    yggdrasil::OnlineMode::new(
                                        yggdrasil::UserInfo {
                                            name: Cow::Borrowed(name),
                                            id,
                                        },
                                        auth,
                                    ),
                                    Client::NO_LOGIN_PLUGIN_HANDLER,
                                )
                                .await?;
                        }
                        AuthConfig::Microsoft(auth, _) => {
                            client
                                .login(
                                    Some(web_client.clone()),
                                    microsoft::OnlineMode::new(
                                        microsoft::UserInfo {
                                            name: Cow::Borrowed(name),
                                            id,
                                        },
                                        auth,
                                    ),
                                    Client::NO_LOGIN_PLUGIN_HANDLER,
                                )
                                .await?;
                        }
                    };

                    let Client {
                        inbound,
                        outbound,
                        version,
                        ..
                    } = &mut client;
                    let version = *version;
                    let send_to_client = Mutex::new(Vec::new());
                    let all_local_players =
                        Mutex::new(BiHashMap::<UUID, Ascii<Cow<'_, str>>>::new());
                    let pings = Mutex::new(HashMap::<UUID, i32>::new());
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
                                        if let Some(unames) = msg.strip_prefix("/stats ") {
                                            let players = if unames == "*" {
                                                all_local_players
                                                    .lock()
                                                    .unwrap()
                                                    .iter()
                                                    .map(|(&uuid, v)| {
                                                        (Some(uuid), Cow::Owned(v.as_ref().into()))
                                                    })
                                                    .collect::<Vec<_>>()
                                            } else {
                                                unames
                                                    .split(' ')
                                                    .map(|uname| {
                                                        (
                                                            all_local_players
                                                                .lock()
                                                                .unwrap()
                                                                .get_by_right(&Ascii::new(
                                                                    uname.into(),
                                                                ))
                                                                .copied(),
                                                            Cow::Borrowed(uname),
                                                        )
                                                    })
                                                    .collect()
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
                                                            uuid = mojang
                                                                .get_uuid(&player)
                                                                .await
                                                                .ok()
                                                                .map(|(uuid, name)| {
                                                                    player = name;
                                                                    uuid
                                                                });
                                                        }
                                                        let mut player_info = None;
                                                        let mut nicked = true;
                                                        if let Some(uuid) = uuid {
                                                            if let Some(info) =
                                                                hypixel.info(uuid).await?
                                                            {
                                                                nicked = false;
                                                                let stats = &info.stats;
                                                                if let Some(bw_stats) =
                                                                    &stats.bedwars
                                                                {
                                                                    let fkdr = bw_stats
                                                                        .final_kills
                                                                        .map_or(0.0, |v| v as f64)
                                                                        / bw_stats
                                                                            .final_deaths
                                                                            .map_or(1.0, |v| {
                                                                                v as f64
                                                                            });
                                                                    if fkdr > 2.0 {
                                                                        out.push(
                                                                            format!(
                                                                                "has {:.2} FKDR",
                                                                                fkdr,
                                                                            )
                                                                            .into(),
                                                                        );
                                                                    }
                                                                    display.push(Chat::Raw(
                                                                        format!("{:.2} FKDR", fkdr)
                                                                            .into(),
                                                                    ));
                                                                }
                                                                player_info = Some(info);
                                                            } else if uuid_lookup
                                                                || mojang
                                                                    .get_uuid(&player)
                                                                    .await
                                                                    .is_ok()
                                                            {
                                                                nicked = false;
                                                            }
                                                        }
                                                        if nicked {
                                                            out.push("is nicked".into());
                                                        }
                                                        let out = if out.is_empty() {
                                                            None
                                                        } else {
                                                            Some(format!(
                                                                "{} {}",
                                                                &player,
                                                                out.join(", ")
                                                            ))
                                                        };
                                                        send_to_client.lock().unwrap().push(
                                                            if let Some(ref player_info) =
                                                                player_info
                                                            {
                                                                Chat::Array(vec![
                                                                    player_info.into(),
                                                                    Chat::Object(ChatObject {
                                                                        color: Some(Color::Reset),
                                                                        value: ChatValue::Text {
                                                                            text: ": ".into(),
                                                                        },
                                                                        extra: Some(
                                                                            display
                                                                                .into_iter()
                                                                                .intersperse(
                                                                                    Chat::Raw(
                                                                                        ", ".into(),
                                                                                    ),
                                                                                )
                                                                                .collect(),
                                                                        ),
                                                                        ..Default::default()
                                                                    }),
                                                                ])
                                                            } else {
                                                                Chat::Object(ChatObject {
                                                                    color: Some(if nicked {
                                                                        Color::DarkRed
                                                                    } else {
                                                                        Color::Gray
                                                                    }),
                                                                    value: ChatValue::Text {
                                                                        text: (if nicked {
                                                                            "[NICKED] "
                                                                        } else {
                                                                            ""
                                                                        })
                                                                        .into(),
                                                                    },
                                                                    extra: Some(vec![
                                                                        Chat::Raw(player.into()),
                                                                        Chat::Raw(
                                                                            "§r: Unknown".into(),
                                                                        ),
                                                                    ]),
                                                                    ..Default::default()
                                                                })
                                                            },
                                                        );
                                                        Ok::<
                                                            Option<String>,
                                                            Box<dyn Error + Send + Sync + 'static>,
                                                        >(
                                                            out
                                                        )
                                                    }
                                                }))
                                                .await;
                                            if unames == "*" {
                                                for good_player in good_players {
                                                    if let Some(msg) = good_player? {
                                                        let mut out_packet =
                                                            outbound.create_packet(1, None).await?;
                                                        LengthCappedString::<256>(
                                                            format!("/pc {}", msg).into(),
                                                        )
                                                        .encode(&mut out_packet, version)
                                                        .await?;
                                                        out_packet.shutdown().await?;
                                                    }
                                                }
                                            }
                                        } else if msg == "/ping" || msg.starts_with("/ping ") {
                                            let unames = &msg[5..];
                                            let players = if unames == "" {
                                                vec![(Some(id), Cow::Borrowed(name))]
                                            } else if unames == " *" {
                                                all_local_players
                                                    .lock()
                                                    .unwrap()
                                                    .iter()
                                                    .map(|(&uuid, v)| {
                                                        (Some(uuid), Cow::Owned(v.as_ref().into()))
                                                    })
                                                    .collect::<Vec<_>>()
                                            } else {
                                                unames
                                                    .split(' ')
                                                    .skip(1)
                                                    .map(|uname| {
                                                        (all_local_players
                                                            .lock()
                                                            .unwrap()
                                                            .get_by_right(&Ascii::new(
                                                                uname.into(),
                                                            ))
                                                            .copied(),
                                                        Cow::Borrowed(uname))
                                                    })
                                                    .collect()
                                            };
                                            let results = join_all(players.into_iter().map(|player| {
                                                let hypixel = &hypixel;
                                                let send_to_client = &send_to_client;
                                                let pings = &pings;
                                                let mojang = &mojang;
                                                let mut uuid = player.0;
                                                let mut player = player.1.into_owned();
                                                async move {
                                                    if uuid.is_none() {
                                                        uuid = mojang
                                                            .get_uuid(&player)
                                                            .await
                                                            .ok()
                                                            .map(|(uuid, name)| {
                                                                player = name;
                                                                uuid
                                                            });
                                                    }
                                                    let mut ping = None;
                                                    let mut player_info = None;
                                                    if let Some(uuid) = uuid {
                                                        ping = pings.lock().unwrap().get(&uuid).copied();
                                                        player_info = hypixel.info(uuid).await?;
                                                    }
                                                    send_to_client.lock().unwrap().push(
                                                        Chat::Array(vec![
                                                            if let Some(ref player_info) = player_info {
                                                                player_info.into()
                                                            } else {
                                                                Chat::Raw(format!("§4[NICKED] {}", player).into())
                                                            },
                                                            Chat::Object(ChatObject {
                                                                color: Some(Color::Reset),
                                                                value: ChatValue::Text {
                                                                    text: ": ".into(),
                                                                },
                                                                ..Default::default()
                                                            }),
                                                            if let Some(ping) = ping {
                                                                Chat::Object(ChatObject {
                                                                    color: Some(if ping < 50 {
                                                                        Color::DarkGreen
                                                                    } else if ping < 100 {
                                                                        Color::Green
                                                                    } else if ping < 200 {
                                                                        Color::Yellow
                                                                    } else {
                                                                        Color::Red
                                                                    }),
                                                                    value: ChatValue::Text {
                                                                        text: format!("{}ms", ping).into(),
                                                                    },
                                                                    ..Default::default()
                                                                })
                                                            } else {
                                                                Chat::Raw("Unknown".into())
                                                            }
                                                        ])
                                                    );
                                                    Ok::<
                                                        (),
                                                        Box<dyn Error + Send + Sync + 'static>,
                                                    >(())
                                                }
                                            })).await;
                                            for result in results {
                                                result?;
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
                                        let mut out_packet = outbound
                                            .create_packet(packet.id, Some(packet.len))
                                            .await?;
                                        copy(&mut packet.content, &mut out_packet).await?;
                                        packet.content.finished()?;
                                        out_packet.shutdown().await?;
                                    }
                                }
                            }
                            #[allow(unreachable_code)]
                            Ok::<(), Box<dyn Error + Send + Sync + 'static>>(())
                        },
                        async {
                            loop {
                                while let Some(out_chat) = {
                                    let chat = send_to_client.lock().unwrap().pop();
                                    // TODO: figure out why this is needed???
                                    chat
                                } {
                                    let mut out_packet =
                                        server_outbound.create_packet(2, None).await?;
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
                                                    for _ in
                                                        0..VarInt::decode(&mut content, version)
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
                                                        if bool::decode(&mut content, version)
                                                            .await?
                                                        {
                                                            LengthCappedString::<32767>::decode(
                                                                &mut content,
                                                                version,
                                                            )
                                                            .await?;
                                                        }
                                                    }
                                                    VarInt::decode(&mut content, version).await?;
                                                    let ping = VarInt::decode(&mut content, version).await?.0;
                                                    if bool::decode(&mut content, version).await? {
                                                        Chat::decode(&mut content, version).await?;
                                                    }
                                                    pings.lock().unwrap().insert(uuid, ping);
                                                    all_local_players
                                                        .lock()
                                                        .unwrap()
                                                        .insert(uuid, Ascii::new(name));
                                                }
                                                2 => {
                                                    let ping = VarInt::decode(&mut content, version).await?.0;
                                                    pings.lock().unwrap().insert(uuid, ping);
                                                }
                                                4 => {
                                                    all_local_players
                                                        .lock()
                                                        .unwrap()
                                                        .remove_by_left(&uuid);
                                                    pings.lock().unwrap().remove(&uuid);
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
                            Ok::<(), Box<dyn Error + Send + Sync + 'static>>(())
                        }
                    )?;
                }
                Ok::<(), Box<dyn Error + Send + Sync + 'static>>(())
            }
            .await
            {
                println!("Connection closed: {}", err);
            }
        });
    }
}
