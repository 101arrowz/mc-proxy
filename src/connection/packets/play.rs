use crate::connection::{
    codec::{IncomingPacket, OutgoingPacket},
    error::Error,
    Client, ServerConnection, State,
};
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
            let to_client: Rc<RefCell<Vec<Chat>>> = Rc::new(RefCell::new(Vec::new()));
            let send_to_client = to_client.clone();
            let ServerConnection {
                inbound: server_inbound,
                outbound: server_outbound,
                version: server_version,
                ..
            } = self;
            let server_version = *server_version;
            try_join!(
                async {
                    loop {
                        let mut packet = server_inbound.next_packet().await?;
                        match packet.id {
                            15 => {
                                packet.content.close().await?;
                                to_client.borrow_mut().push(Chat::Raw("hello world".into()));
                            }
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
                        for out_chat in send_to_client.borrow_mut().drain(..) {
                            let mut out_packet = server_outbound.create_packet(15, None).await?;
                            out_chat.encode(&mut out_packet, version).await?;
                            1u8.encode(&mut out_packet, version).await?;
                            UUID(0).encode(&mut out_packet, version).await?;
                            out_packet.shutdown().await?;
                        }
                        let mut packet = inbound.next_packet().await?;
                        let mut out_packet = server_outbound.create_packet(packet.id, Some(packet.len)).await?;
                        copy(&mut packet.content, &mut out_packet).await?;
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
