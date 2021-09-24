use crate::connection::{error::Error, Client, State};
use crate::protocol::{
    types::{Encode, Decode, LengthCappedString, VarInt, UUID, Chat},
    version::ProtocolVersion,
    error::Error as ProtocolError
};
use std::{borrow::Cow, convert::TryInto, future::Future, io::Cursor};
use sha1::{Sha1, Digest};
use rand::{thread_rng, Rng};
use rsa::{PublicKey, RsaPublicKey, PaddingScheme, pkcs1::FromRsaPublicKey};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use reqwest::Client as HTTPClient;
use serde::{Serialize, Deserialize};
use tokio::try_join;

#[derive(Debug, Clone)]
pub struct LoginCredentials<'a> {
    access_token: &'a str,
    uuid: UUID,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FullLoginCredentials<'a> {
    access_token: &'a str,
    uuid: UUID,
    server_id: &'a str
}

pub trait Authenticator {
    type CredentialsOutput<'a>: Future<Output = Result<LoginCredentials<'a>, Error>>;
    fn username(&self) -> &str;
    fn credentials(&self) -> Self::CredentialsOutput<'_>;
}

impl Client {
    pub async fn login(&mut self, client: HTTPClient, authenticator: impl Authenticator) -> Result<(), Error> {
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
                    }
                    1 => {
                        let server_id = LengthCappedString::<20>::decode(&mut packet.content, self.version).await?;
                        let mut public_key_bytes = vec![0; VarInt::decode(&mut packet.content, self.version).await?.0 as usize];
                        packet.content.read_exact(&mut public_key_bytes).await?;
                        let mut verify_token = vec![0; VarInt::decode(&mut packet.content, self.version).await?.0 as usize];
                        packet.content.read_exact(&mut verify_token).await?;

                        let mut hasher = Sha1::new();
                        let mut rng = thread_rng();
                        let mut shared_secret = [0; 16];
                        rng.fill(&mut shared_secret);
                        hasher.update(server_id.0.as_bytes());
                        hasher.update(shared_secret);
                        hasher.update(&public_key_bytes);
                        let output = hasher.finalize();

                        let credentials = authenticator.credentials().await?;
                        client.post("https://sessionserver.mojang.com/session/minecraft/join")
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
                        self.inbound.conn.set_key(Some(shared_secret));
                        self.outbound.conn.set_key(Some(shared_secret));
                    },
                    4 => {
                        todo!();
                    },
                    _ => Err(ProtocolError::Malformed)?
                }
            }
        } else {
            Err(Error::InvalidState)
        }
    }
}
