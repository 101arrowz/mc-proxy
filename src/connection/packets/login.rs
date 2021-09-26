use crate::connection::{error::Error, Client, State};
use crate::protocol::{
    types::{Encode, Decode, LengthCappedString, Identifier, VarInt, UUID, Chat, serde_raw_uuid},
    version::ProtocolVersion,
    error::Error as ProtocolError
};
use std::{borrow::Cow, convert::TryInto, future::{Future, Ready, ready}, io::Cursor};
use sha1::{Sha1, Digest};
use rand::{thread_rng, Rng};
use rsa::{PublicKey, RsaPublicKey, PaddingScheme, pkcs1::FromRsaPublicKey};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use reqwest::Client as HTTPClient;
use serde::{Serialize, Deserialize};

#[derive(Debug, Clone)]
pub struct LoginCredentials<'a> {
    pub access_token: &'a str,
    pub uuid: UUID,
}

pub trait Authenticator<'a> {
    type CredentialsOutput: Future<Output = Result<LoginCredentials<'a>, Error>>;
    fn username(&mut self) -> &str;
    fn credentials(&mut self) -> Self::CredentialsOutput;
}

#[derive(Debug, Clone)]
pub struct LoginResponse<'a> {
    pub username: Cow<'a, str>,
    pub uuid: UUID
}

impl Client {
    pub async fn login<'a, P: Future<Output = Option<impl AsRef<[u8]>>>>(&mut self, mut client: Option<HTTPClient>, mut authenticator: impl Authenticator<'_>, mut plugin_handler: Option<impl FnMut(Cow<'a, str>, Vec<u8>) -> P>) -> Result<LoginResponse<'_>, Error> {
        if self.state == State::Login {
            let username = authenticator.username();
            let start_packet_len = username.len() + 1;
            let username = LengthCappedString::<16>(Cow::Borrowed(username));
            username.encode(&mut self.outbound.create_packet(0, Some(start_packet_len)).await?, self.version).await?;
            loop {
                let mut packet = self.inbound.next_packet().await?;
                match packet.id {
                    0 => {
                        break Err(Error::Disconnected(
                            Chat::decode(&mut packet.content, self.version).await?.into_owned()
                        ));
                    },
                    1 => {
                        let server_id = LengthCappedString::<20>::decode(&mut packet.content, self.version).await?;
                        let mut public_key_bytes = vec![0; VarInt::decode(&mut packet.content, self.version).await?.0 as usize];
                        packet.content.read_exact(&mut public_key_bytes).await?;
                        let mut verify_token = vec![0; VarInt::decode(&mut packet.content, self.version).await?.0 as usize];
                        packet.content.read_exact(&mut verify_token).await?;
                        packet.content.finished()?;

                        let mut hasher = Sha1::new();
                        let mut rng = thread_rng();
                        let mut shared_secret = [0; 16];
                        rng.fill(&mut shared_secret);
                        hasher.update(server_id.0.as_bytes());
                        hasher.update(shared_secret);
                        hasher.update(&public_key_bytes);
                        let output = hasher.finalize();

                        let credentials = authenticator.credentials().await?;

                        #[derive(Debug, Clone, Serialize, Deserialize)]
                        #[serde(rename_all = "camelCase")]
                        struct FullLoginCredentials<'a> {
                            access_token: &'a str,
                            #[serde(with = "serde_raw_uuid")]
                            uuid: UUID,
                            server_id: &'a str
                        }

                        client.take().unwrap_or_else(HTTPClient::new).post("https://sessionserver.mojang.com/session/minecraft/join")
                            .json(&FullLoginCredentials {
                                access_token: credentials.access_token,
                                uuid: credentials.uuid,
                                server_id: std::str::from_utf8(&output).unwrap()
                            })
                            .send()
                            .await?
                            .error_for_status()?;

                        let public_key = RsaPublicKey::from_pkcs1_der(&public_key_bytes).map_err(|_| ProtocolError::Malformed)?;
                        let encrypted_shared_secret = public_key.encrypt(&mut rng, PaddingScheme::PKCS1v15Encrypt, &shared_secret).map_err(|_| ProtocolError::Malformed)?;
                        let encrypted_verify_token = public_key.encrypt(&mut rng, PaddingScheme::PKCS1v15Encrypt, &verify_token).map_err(|_| ProtocolError::Malformed)?;

                        let encrypted_shared_secret_len = VarInt(encrypted_shared_secret.len() as i32);
                        let encrypted_verify_token_len = VarInt(encrypted_verify_token.len() as i32);

                        let mut response_packet = self.outbound.create_packet(1, Some(
                            encrypted_shared_secret_len.len() +
                            encrypted_shared_secret.len() + 
                            encrypted_verify_token_len.len() + 
                            encrypted_verify_token.len()
                        )).await?;

                        encrypted_shared_secret_len.encode(&mut response_packet, self.version).await?;
                        response_packet.write_all(&encrypted_shared_secret).await?;
                        encrypted_verify_token_len.encode(&mut response_packet, self.version).await?;
                        response_packet.write_all(&encrypted_verify_token).await?;
                        response_packet.shutdown().await?;
                        if !self.inbound.conn.set_key(shared_secret) || !self.outbound.conn.set_key(shared_secret) {
                            Err(ProtocolError::Malformed)?;
                        }
                    },
                    2 => {
                        let response = LoginResponse {
                            uuid: UUID::decode(&mut packet.content, self.version).await?,
                            username: LengthCappedString::<16>::decode(&mut packet.content, self.version).await?.0
                        };
                        packet.content.finished()?;
                        self.state = State::Play;
                        break Ok(response);
                    },
                    3 => {
                        let new_threshold = VarInt::decode(&mut packet.content, self.version).await?.0 as usize;
                        packet.content.finished()?;
                        self.inbound.compressed = true;
                        self.outbound.compress_threshold = Some(new_threshold);
                    },
                    4 => {
                        let message_id = VarInt::decode(&mut packet.content, self.version).await?;
                        if let Some(ref mut plugin_handler) = plugin_handler {
                            let channel = Identifier::decode(&mut packet.content, self.version).await?.0;
                            let bytes_remaining = packet.len - message_id.len() - channel.len() - VarInt(channel.len() as i32).len();
                            let mut buf = Vec::with_capacity(bytes_remaining);
                            packet.content.read_to_end(&mut buf).await?;
                            if buf.len() != bytes_remaining {
                                Err(ProtocolError::Malformed)?
                            }
                            if let Some(response_data) = plugin_handler(channel, buf).await {
                                let response_data = response_data.as_ref();
                                let mut response_packet = self.outbound.create_packet(2, Some(
                                    message_id.len() + 1 + response_data.len()
                                )).await?;
                                message_id.encode(&mut response_packet, self.version).await?;
                                (true).encode(&mut response_packet, self.version).await?;
                                response_packet.write_all(response_data).await?;
                                response_packet.shutdown().await?;
                            } else {
                                let mut response_packet = self.outbound.create_packet(2, Some(
                                    message_id.len() + 1
                                )).await?;
                                message_id.encode(&mut response_packet, self.version).await?;
                                (false).encode(&mut response_packet, self.version).await?;
                                response_packet.shutdown().await?;
                            }
                        } else {
                            packet.content.close().await?;
                            let mut response_packet = self.outbound.create_packet(2, Some(
                                message_id.len() + 1
                            )).await?;
                            message_id.encode(&mut response_packet, self.version).await?;
                            (false).encode(&mut response_packet, self.version).await?;
                            response_packet.shutdown().await?;
                        }
                    },
                    _ => Err(ProtocolError::Malformed)?
                }
            }
        } else {
            Err(Error::InvalidState)
        }
    }
}

pub struct OfflineMode<'a>(pub &'a str);

impl<'a> Authenticator<'a> for OfflineMode<'a> {
    type CredentialsOutput = Ready<Result<LoginCredentials<'a>, Error>>;

    fn username(&mut self) -> &str {
        self.0
    }

    fn credentials(&mut self) -> Self::CredentialsOutput {
        ready(Err(Error::NoCredentials))
    }
}

pub struct OnlineMode<'a> {
    pub username: &'a str,
    pub password: &'a str,
    pub client: Option<HTTPClient>,
    pub client_token: Option<&'a str>
} 

impl<'a> Authenticator<'a> for OnlineMode<'a> {
    type CredentialsOutput = impl Future<Output = Result<LoginCredentials<'a>, Error>>;

    fn username(&mut self) -> &str {
        self.username
    }

    fn credentials(&mut self) -> Self::CredentialsOutput {
        if self.client.is_none() {
            self.client = Some(HTTPClient::new());
        }
        let client = self.client.clone().unwrap();
        async {
            struct AuthenticationRequest {

            }
            struct AuthenticationResponse {
                
            }
            client.post("https://authserver.mojang.com/authenticate")
                .json()
            Err(Error::IncompletePacket)

        }
    }
}