use crate::{connection::{
    codec::{IncomingPacket, OutgoingPacket},
    error::Error,
    Client, ServerConnection, State,
}, protocol::types::{Decode, LengthCappedString}};
use crate::protocol::{types::{Encode, Chat, UUID}, version::ProtocolVersion};
use std::{
    convert::Infallible,
    future::{ready, Future, Ready},
    rc::Rc,
    cell::RefCell
};
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
            let ServerConnection {
                inbound: server_inbound,
                outbound: server_outbound,
                version: server_version,
                ..
            } = self;
            let server_version = *server_version;
            try_join!(
                async {
                    // loop {
                    //     tokio::time::sleep(tokio::time::Duration::from_millis(10000)).await;
                    //     dbg!("sending e");
                    //     let mut out_packet = outbound.create_packet(1, Some(51)).await?;
                    //     LengthCappedString::<256>("i am a script that sends this message every second".into()).encode(&mut out_packet, version).await?;
                    //     out_packet.shutdown().await?;
                    //     dbg!("sent e");
                    // }
                    loop {
                        let mut packet = server_inbound.next_packet().await?;
                        match packet.id {
                            1 => {
                                let msg = LengthCappedString::<256>::decode(&mut packet.content, server_version).await?;
                                packet.content.finished()?;
                                if msg.0 == "/hello_world" {
                                    send_to_client.borrow_mut().push(Chat::Raw("hello world!".into()));
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
                        let mut buf = Vec::new();
                        packet.content.read_to_end(&mut buf).await?;
                        // let (_, _, buf) = dbg!(packet.id, packet.len, buf);
                        let mut out_packet = server_outbound.create_packet(packet.id, Some(packet.len)).await?;
                        // copy(&mut packet.content, &mut out_packet).await?;
                        out_packet.write_all(&buf).await?;
                        packet.content.finished()?;
                        out_packet.shutdown().await?;
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
