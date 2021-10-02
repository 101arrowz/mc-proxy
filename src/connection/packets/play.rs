use crate::{connection::{
    codec::{IncomingPacket, OutgoingPacket},
    error::Error,
    Client, ServerConnection, State,
}, protocol::types::{Decode, LengthCappedString, VarInt}};
use crate::protocol::{types::{Encode, Chat, UUID}, version::ProtocolVersion};
use std::{
    rc::Rc,
    cell::RefCell,
    borrow::Cow,
    collections::HashMap,
    io::Cursor
};
use futures::future::join_all;
use tokio::{
    io::{copy, AsyncReadExt, AsyncWriteExt},
    net::tcp::{OwnedReadHalf, OwnedWriteHalf},
    try_join,
};

// pub struct PacketInterceptor<E> {
//     pub id: i32,
//     pub len: Option<usize>,
//     pub writer: E,
// }

// pub enum PacketInterceptionMode<E> {
//     None,
//     Discard,
//     Intercept(PacketInterceptor<E>),
// }

// impl ServerConnection {
//     pub const NO_PLAY_PROXY_FILTER: fn(
//         &mut IncomingPacket<OwnedReadHalf>,
//         ProtocolVersion,
//         ProtocolVersion,
//     ) -> Ready<
//         PacketInterceptionMode<
//             fn(&mut OutgoingPacket<'_, OwnedWriteHalf>) -> Ready<Result<(), Error>>,
//         >,
//     > = |_, _, _| ready(PacketInterceptionMode::None);
//     pub const PACKET_INTERCEPT_NONE: PacketInterceptionMode<fn(&mut OutgoingPacket<'_, OwnedWriteHalf>) -> Ready<Result<(), Error>>> = PacketInterceptionMode::None;
//     pub const PACKET_INTERCEPT_DISCARD: PacketInterceptionMode<fn(&mut OutgoingPacket<'_, OwnedWriteHalf>) -> Ready<Result<(), Error>>> = PacketInterceptionMode::Discard;
//     pub async fn play_proxy<
//         ICR: Future<Output = Result<(), Error>>,
//         ICF: FnOnce(&mut OutgoingPacket<OwnedWriteHalf>) -> ICR,
//         IC: Future<
//             Output = PacketInterceptionMode<
//                 ICF
//             >,
//         >,
//         FC: FnMut(
//             &mut IncomingPacket<OwnedReadHalf>,
//             ProtocolVersion,
//             ProtocolVersion,
//         ) -> IC,
//         ISR: Future<Output = Result<(), Error>>,
//         ISF: FnOnce(&mut OutgoingPacket<OwnedWriteHalf>) -> ISR,
//         IS: Future<
//             Output = PacketInterceptionMode<
//                 ISF,
//             >,
//         >,
//         FS: FnMut(
//             &mut IncomingPacket<OwnedReadHalf>,
//             ProtocolVersion,
//             ProtocolVersion,
//         ) -> IS
//     >(
//         &mut self,
//         client: &mut Client,
//         mut filter_client: FC,
//         mut filter_server: FS
//     ) -> Result<(), Error> {
//         if self.state == State::Play {
//             let Client {
//                 inbound,
//                 outbound,
//                 version,
//                 ..
//             } = client;
//             let version = *version;
//             let ServerConnection {
//                 inbound: server_inbound,
//                 outbound: server_outbound,
//                 version: server_version,
//                 ..
//             } = self;
//             let server_version = *server_version;
//             try_join!(
//                 async {
//                     loop {
//                         let mut packet = server_inbound.next_packet().await?;
//                         let mode = (filter_client)(&mut packet, server_version, version).await;
//                         match mode {
//                             PacketInterceptionMode::Intercept(intercept) => {
//                                 packet.content.close().await?;
//                                 let mut out_packet =
//                                     outbound.create_packet(intercept.id, intercept.len).await?;
//                                 (intercept.writer)(&mut out_packet).await?;
//                                 out_packet.shutdown().await?;
//                             }
//                             PacketInterceptionMode::Discard => {
//                                 packet.content.close().await?;
//                             }
//                             PacketInterceptionMode::None => {
//                                 let mut out_packet =
//                                     outbound.create_packet(packet.id, Some(packet.len)).await?;
//                                 // TODO: optimize
//                                 copy(&mut packet.content, &mut out_packet).await?;
//                                 packet.content.finished()?;
//                                 out_packet.shutdown().await?;
//                             }
//                         }
//                     }
//                     Ok::<(), Error>(())
//                 },
//                 async {
//                     loop {
//                         let mut packet = inbound.next_packet().await?;
//                         let mode = (filter_server)(&mut packet, server_version, version).await;
//                         match mode {
//                             PacketInterceptionMode::Intercept(intercept) => {
//                                 packet.content.close().await?;
//                                 let mut out_packet = server_outbound
//                                     .create_packet(intercept.id, intercept.len)
//                                     .await?;
//                                 (intercept.writer)(&mut out_packet).await?;
//                                 out_packet.shutdown().await?;
//                             }
//                             PacketInterceptionMode::Discard => {
//                                 packet.content.close().await?;
//                             }
//                             PacketInterceptionMode::None => {
//                                 let mut out_packet = server_outbound
//                                     .create_packet(packet.id, Some(packet.len))
//                                     .await?;
//                                 // TODO: optimize
//                                 copy(&mut packet.content, &mut out_packet).await?;
//                                 packet.content.finished()?;
//                                 out_packet.shutdown().await?;
//                             }
//                         }
//                     }
//                     Ok::<(), Error>(())
//                 }
//             )
//             .and(Ok(()))
//         } else {
//             Err(Error::InvalidState)
//         }
//     }
// }

impl ServerConnection {
    pub async fn play_proxy(
        &mut self,
        client: &mut Client
    ) -> Result<(), Error> {
        if self.state == State::Play {
            let Client {
                inbound,
                outbound,
                version,
                ..
            } = client;
            let version = *version;
            let send_to_client = Rc::new(RefCell::new(Vec::new()));
            let all_local_players = Rc::new(RefCell::new(HashMap::<UUID, String>::new()));
            let ServerConnection {
                inbound: server_inbound,
                outbound: server_outbound,
                version: server_version,
                ..
            } = self;
            let server_version = *server_version;
            let client = reqwest::Client::new();
            try_join!(
                async {
                    loop {
                        let mut packet = server_inbound.next_packet().await?;
                        match packet.id {
                            1 => {
                                let msg = LengthCappedString::<256>::decode(&mut packet.content, server_version).await?;
                                packet.content.finished()?;
                                if msg.0.starts_with("/stats ") {
                                    let uname = &msg.0[7..];
                                    let players = if uname == "*" {
                                        all_local_players.borrow().iter().map(|(_, v)| Cow::Owned(v.into())).collect()
                                    } else {
                                        Vec::from([Cow::Borrowed(uname)])
                                    };
                                    let good_players = join_all(players.into_iter().map(|player| {
                                        let client = &client;
                                        let send_to_client = send_to_client.clone();
                                        async move {
                                            let mut out = None;
                                            let mut result: Cow<'_, str>;
                                            // todo: optimize
                                            let resp: serde_json::Value = client.clone()
                                                .get(["https://api.mojang.com/users/profiles/minecraft/", &player].concat())
                                                .send().await?
                                                .json().await.unwrap_or_default();
                                            if let Some(uuid) = resp.get("id") {
                                                let uuid = uuid.as_str().unwrap();
                                                let resp: serde_json::Value = client
                                                    .get("https://api.hypixel.net/player")
                                                    .query(&[("uuid", uuid)])
                                                    .header("API-Key", "defunct_key")
                                                    .send().await?
                                                    .json().await?;
                                                result = "Unknown".into();
                                                if let Some(player_info) = resp.get("player") {
                                                    let bw_stats = &player_info["stats"]["Bedwars"];
                                                    if bw_stats != &serde_json::Value::Null {
                                                        let fkdr = bw_stats["final_kills_bedwars"].as_f64().unwrap_or(0.0) / bw_stats["final_deaths_bedwars"].as_f64().unwrap_or(1.0);
                                                        if fkdr > 3.5 {
                                                            out = Some((fkdr, player.to_string()));
                                                        }
                                                        result = format!("{:.2}", fkdr).into();
                                                    }
                                                }
                                            } else {
                                                result = "Nicked".into();
                                                out = Some((-1.0, player.to_string()));
                                            }
                                            send_to_client.borrow_mut().push(Chat::Raw([&player, ": ", &result].concat().into()));
                                            Ok::<Option<(f64, String)>, Error>(out)
                                        }
                                    })).await;
                                    if uname == "*" {
                                        for good_player in good_players {
                                            if let Some((fkdr, name)) = good_player? {
                                                let mut out_packet = outbound.create_packet(1, None).await?;
                                                let warning = if fkdr < 0.0 {
                                                    format!("[QDodge Bot] {} is nicked", name)
                                                } else {
                                                    format!("[QDodge Bot] {} has {:.2} FKDR", name, fkdr)
                                                };
                                                LengthCappedString::<256>(warning.into()).encode(&mut out_packet, version).await?;
                                                out_packet.shutdown().await?;
                                            }
                                        }
                                    }
                                } else {  
                                    let mut out_packet = outbound.create_packet(packet.id, Some(packet.len)).await?;
                                    msg.encode(&mut out_packet, version).await?;
                                    out_packet.shutdown().await?;
                                }
                                
                            },
                            _ => {
                                let mut out_packet = outbound.create_packet(packet.id, Some(packet.len)).await?;
                                copy(&mut packet.content, &mut out_packet).await?;
                                packet.content.finished()?;
                                out_packet.shutdown().await?;
                            }
                        }
                    }
                    Ok::<(), Error>(())
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
                                let num_players = VarInt::decode(&mut content, version).await?.0;
                                for _ in 0..num_players {
                                    let uuid = UUID::decode(&mut content, version).await?;
                                    match action {
                                        0 => {
                                            let name = LengthCappedString::<16>::decode(&mut content, version).await?.0;
                                            all_local_players.borrow_mut().insert(uuid, name.into());
                                            for _ in 0..VarInt::decode(&mut content, version).await?.0 {
                                                LengthCappedString::<32767>::decode(&mut content, version).await?;
                                                LengthCappedString::<32767>::decode(&mut content, version).await?;
                                                if bool::decode(&mut content, version).await? {
                                                    LengthCappedString::<32767>::decode(&mut content, version).await?;
                                                }
                                            }
                                            VarInt::decode(&mut content, version).await?;
                                            VarInt::decode(&mut content, version).await?;
                                            if bool::decode(&mut content, version).await? {
                                                Chat::decode(&mut content, version).await?;
                                            }
                                        },
                                        4 => {
                                            all_local_players.borrow_mut().remove(&uuid);
                                        },
                                        _ => {

                                        }
                                    }
                                }
                                content.set_position(0);
                                let mut out_packet = server_outbound.create_packet(packet.id, Some(packet.len)).await?;
                                copy(&mut content, &mut out_packet).await?;
                                packet.content.finished()?;
                                out_packet.shutdown().await?;
                            },
                            _ => {
                                let mut out_packet = server_outbound.create_packet(packet.id, Some(packet.len)).await?;
                                copy(&mut packet.content, &mut out_packet).await?;
                                packet.content.finished()?;
                                out_packet.shutdown().await?;
                            }
                        }
                    }
                    Ok::<(), Error>(())
                }
            )?;
            Ok(())
        } else {
            Err(Error::InvalidState)
        }
    }
}
