use crate::connection::{error::Error, Client, ServerConnection, State};
use crate::protocol::{
    error::Error as ProtocolError,
    types::{serde_raw_uuid, Chat, Decode, Encode, Identifier, LengthCappedString, VarInt, UUID},
    version::ProtocolVersion,
};
use rand::{thread_rng, Rng};
use reqwest::Client as HTTPClient;
use rsa::{
    pkcs1::{der::Decodable, FromRsaPublicKey},
    pkcs8::{ObjectIdentifier, SubjectPublicKeyInfo},
    PaddingScheme, PublicKey, RsaPublicKey,
};
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
use std::{
    borrow::Cow,
    future::{ready, Future, Ready},
    io::Cursor,
    str::from_utf8_unchecked,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Debug, Clone)]
pub struct LoginCredentials<'a> {
    pub access_token: &'a str,
    pub uuid: UUID,
}

pub trait Authenticator {
    type CredentialsOutput<'a>: Future<Output = Result<LoginCredentials<'a>, Error>>;
    fn username(&mut self) -> &str;
    fn credentials(&mut self) -> Self::CredentialsOutput<'_>;
}

#[derive(Debug, Clone)]
pub struct Player<'a> {
    pub username: Cow<'a, str>,
    pub uuid: UUID,
}

impl Client {
    pub const NO_LOGIN_PLUGIN_HANDLER: fn(Cow<str>, Vec<u8>) -> Ready<Option<Vec<u8>>> =
        |_, _| ready(None);
    pub async fn login<'a, P: Future<Output = Option<impl AsRef<[u8]>>>>(
        &mut self,
        mut client: Option<HTTPClient>,
        mut authenticator: impl Authenticator,
        mut plugin_handler: impl FnMut(Cow<'a, str>, Vec<u8>) -> P,
    ) -> Result<Player<'_>, Error> {
        if self.state == State::Login {
            let username = authenticator.username();
            let start_packet_len = username.len() + 1;
            let username = LengthCappedString::<16>(Cow::Borrowed(username));
            username
                .encode(
                    &mut self
                        .outbound
                        .create_packet(0, Some(start_packet_len))
                        .await?,
                    self.version,
                )
                .await?;
            loop {
                let mut packet = self.inbound.next_packet().await?;
                match packet.id {
                    0 => {
                        break Err(Error::Disconnected(
                            Chat::decode(&mut packet.content, self.version)
                                .await?
                                .into_owned(),
                        ));
                    }
                    1 => {
                        let server_id =
                            LengthCappedString::<20>::decode(&mut packet.content, self.version)
                                .await?;
                        let mut public_key_bytes = vec![
                            0;
                            VarInt::decode(&mut packet.content, self.version).await?.0
                                as usize
                        ];
                        packet.content.read_exact(&mut public_key_bytes).await?;
                        let mut verify_token = vec![
                            0;
                            VarInt::decode(&mut packet.content, self.version).await?.0
                                as usize
                        ];
                        packet.content.read_exact(&mut verify_token).await?;
                        packet.content.finished()?;
                        let mut hasher = Sha1::new();
                        let mut rng = thread_rng();
                        let mut shared_secret = [0; 16];
                        rng.fill(&mut shared_secret);
                        hasher.update(server_id.0.as_bytes());
                        hasher.update(shared_secret);
                        hasher.update(&public_key_bytes);
                        let mut server_id_raw = hasher.finalize();
                        let server_id_neg = server_id_raw[0] > 127;
                        if server_id_neg {
                            for val in server_id_raw.iter_mut() {
                                *val ^= 0xFF;
                            }
                            for val in server_id_raw.iter_mut().rev() {
                                if *val == 0xFF {
                                    *val = 0;
                                } else {
                                    *val += 1;
                                    break;
                                }
                            }
                        }
                        let mut server_id = [0; 41];
                        let server_id_index_start: usize;
                        let mut output_iter = server_id_raw
                            .iter()
                            .flat_map(|&v| [v >> 4, v & 15])
                            .enumerate();
                        if let Some((first_nonzero, first_nonzero_val)) =
                            output_iter.find(|&(_, v)| v != 0)
                        {
                            for (i, hex) in output_iter {
                                server_id[i + 1] = hex + (if hex < 10 { b'0' } else { b'a' - 10 });
                            }
                            server_id[first_nonzero + 1] = first_nonzero_val
                                + (if first_nonzero_val < 10 {
                                    b'0'
                                } else {
                                    b'a' - 10
                                });
                            if server_id_neg {
                                server_id_index_start = first_nonzero;
                                server_id[server_id_index_start] = b'-';
                            } else {
                                server_id_index_start = first_nonzero + 1;
                            }
                        } else {
                            server_id_index_start = 40;
                            server_id[40] = b'0';
                        }
                        let credentials = authenticator.credentials().await?;

                        #[derive(Debug, Clone, Serialize, Deserialize)]
                        #[serde(rename_all = "camelCase")]
                        struct FullLoginCredentials<'a> {
                            access_token: &'a str,
                            #[serde(with = "serde_raw_uuid")]
                            selected_profile: UUID,
                            server_id: &'a str,
                        }

                        client
                            .take()
                            .unwrap_or_default()
                            .post("https://sessionserver.mojang.com/session/minecraft/join")
                            .json(&FullLoginCredentials {
                                access_token: credentials.access_token,
                                selected_profile: credentials.uuid,
                                server_id: unsafe {
                                    from_utf8_unchecked(&server_id[server_id_index_start..])
                                },
                            })
                            .send()
                            .await?
                            .error_for_status()?;

                        let spki = SubjectPublicKeyInfo::from_der(&public_key_bytes)
                            .map_err(|_| ProtocolError::Malformed)?;
                        const RSA_OID: ObjectIdentifier =
                            ObjectIdentifier::new("1.2.840.113549.1.1.1");
                        if spki.algorithm.oid != RSA_OID {
                            Err(ProtocolError::Malformed)?;
                        }
                        let public_key = RsaPublicKey::from_pkcs1_der(spki.subject_public_key)
                            .map_err(|_| ProtocolError::Malformed)?;
                        let encrypted_shared_secret = public_key
                            .encrypt(&mut rng, PaddingScheme::PKCS1v15Encrypt, &shared_secret)
                            .map_err(|_| ProtocolError::Malformed)?;
                        let encrypted_verify_token = public_key
                            .encrypt(&mut rng, PaddingScheme::PKCS1v15Encrypt, &verify_token)
                            .map_err(|_| ProtocolError::Malformed)?;

                        let encrypted_shared_secret_len =
                            VarInt(encrypted_shared_secret.len() as i32);
                        let encrypted_verify_token_len =
                            VarInt(encrypted_verify_token.len() as i32);

                        let mut response_packet = self
                            .outbound
                            .create_packet(
                                1,
                                Some(
                                    encrypted_shared_secret_len.len()
                                        + encrypted_shared_secret.len()
                                        + encrypted_verify_token_len.len()
                                        + encrypted_verify_token.len(),
                                ),
                            )
                            .await?;

                        encrypted_shared_secret_len
                            .encode(&mut response_packet, self.version)
                            .await?;
                        response_packet.write_all(&encrypted_shared_secret).await?;
                        encrypted_verify_token_len
                            .encode(&mut response_packet, self.version)
                            .await?;
                        response_packet.write_all(&encrypted_verify_token).await?;
                        response_packet.shutdown().await?;
                        if !self.inbound.conn.set_key(shared_secret)
                            || !self.outbound.conn.set_key(shared_secret)
                        {
                            Err(ProtocolError::Malformed)?;
                        }
                    }
                    2 => {
                        let response = Player {
                            uuid: if self.version < ProtocolVersion::V1_16 {
                                LengthCappedString::<36>::decode(&mut packet.content, self.version)
                                    .await?
                                    .0
                                    .parse()?
                            } else {
                                UUID::decode(&mut packet.content, self.version).await?
                            },
                            username: LengthCappedString::<16>::decode(
                                &mut packet.content,
                                self.version,
                            )
                            .await?
                            .0,
                        };
                        packet.content.finished()?;
                        self.state = State::Play;
                        break Ok(response);
                    }
                    3 => {
                        let new_threshold =
                            VarInt::decode(&mut packet.content, self.version).await?.0 as usize;
                        packet.content.finished()?;
                        self.inbound.compressed = true;
                        self.outbound.compress_threshold = Some(new_threshold);
                    }
                    4 => {
                        let message_id = VarInt::decode(&mut packet.content, self.version).await?;
                        let channel = Identifier::decode(&mut packet.content, self.version)
                            .await?
                            .0;
                        let bytes_remaining = packet.len
                            - message_id.len()
                            - channel.len()
                            - VarInt(channel.len() as i32).len();
                        let mut buf = Vec::with_capacity(bytes_remaining);
                        packet.content.read_to_end(&mut buf).await?;
                        if buf.len() != bytes_remaining {
                            Err(ProtocolError::Malformed)?
                        }
                        if let Some(response_data) = plugin_handler(channel, buf).await {
                            let response_data = response_data.as_ref();
                            let mut response_packet = self
                                .outbound
                                .create_packet(2, Some(message_id.len() + 1 + response_data.len()))
                                .await?;
                            message_id
                                .encode(&mut response_packet, self.version)
                                .await?;
                            (true).encode(&mut response_packet, self.version).await?;
                            response_packet.write_all(response_data).await?;
                            response_packet.shutdown().await?;
                        } else {
                            let mut response_packet = self
                                .outbound
                                .create_packet(2, Some(message_id.len() + 1))
                                .await?;
                            message_id
                                .encode(&mut response_packet, self.version)
                                .await?;
                            (false).encode(&mut response_packet, self.version).await?;
                            response_packet.shutdown().await?;
                        }
                    }
                    _ => Err(ProtocolError::Malformed)?,
                }
            }
        } else {
            Err(Error::InvalidState)
        }
    }
}

pub struct OfflineMode<'a>(pub &'a str);

impl Authenticator for OfflineMode<'_> {
    type CredentialsOutput<'a> = Ready<Result<LoginCredentials<'a>, Error>>;

    fn username(&mut self) -> &str {
        self.0
    }

    fn credentials(&mut self) -> Self::CredentialsOutput<'_> {
        ready(Err(Error::NoCredentials))
    }
}

pub enum ServerLoginCredentials<'a> {
    OfflineMode(Player<'a>),
    OnlineMode,
}

impl ServerConnection {
    pub async fn accept_login<'a, M: Future<Output = Result<ServerLoginCredentials<'a>, Error>>>(
        &mut self,
        handler: impl FnOnce(Cow<'_, str>) -> M,
    ) -> Result<Player<'a>, Error> {
        if self.state == State::Login {
            let mut packet = self.inbound.next_packet().await?;
            if packet.id != 0 {
                Err(ProtocolError::Malformed)?;
            }
            let client_username =
                LengthCappedString::<16>::decode(&mut packet.content, self.version)
                    .await?
                    .0;
            let player = match (handler)(client_username).await? {
                ServerLoginCredentials::OfflineMode(player) => player,
                ServerLoginCredentials::OnlineMode => todo!(),
            };
            if self.version < ProtocolVersion::V1_16 {
                let mut out_packet = self.outbound.create_packet(2, Some(player.username.len() + 38)).await?;
                LengthCappedString::<36>(Cow::Borrowed(unsafe { from_utf8_unchecked(&player.uuid.to_ascii_bytes_hyphenated()) }))
                    .encode(&mut out_packet, self.version).await?;
                LengthCappedString::<16>(Cow::Borrowed(&player.username)).encode(&mut out_packet, self.version).await?;
            } else {
                let mut out_packet = self.outbound.create_packet(2, Some(player.username.len() + 17)).await?;
                player.uuid.encode(&mut out_packet, self.version).await?;
                LengthCappedString::<16>(Cow::Borrowed(&player.username)).encode(&mut out_packet, self.version).await?;
            }
            self.state = State::Play;
            Ok(player)
        } else {
            Err(Error::InvalidState)
        }
    }
}
