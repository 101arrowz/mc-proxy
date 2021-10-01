use aes::Aes128;
use cfb8::{
    cipher::{AsyncStreamCipher, NewCipher},
    Cfb8,
};
use std::{
    cmp::max,
    pin::Pin,
    task::{ready, Context, Poll},
};
use tokio::io::{self, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};

type Encryption = Cfb8<Aes128>;

pub struct Encryptor<W: AsyncWrite + Unpin> {
    cipher: Option<Encryption>,
    tgt: W,
    buffer: Box<[u8]>,
    pos: usize,
    cap: usize,
}

const BUFFER_SIZE: usize = 8192;

impl<W: AsyncWrite + Unpin> Encryptor<W> {
    pub fn new(tgt: W) -> Encryptor<W> {
        Encryptor {
            cipher: None,
            tgt,
            buffer: vec![0u8; BUFFER_SIZE].into_boxed_slice(),
            pos: 0,
            cap: 0,
        }
    }

    pub fn set_key(&mut self, key: [u8; 16]) -> bool {
        if self.cipher.is_some() {
            false
        } else {
            self.cipher = Some(Encryption::new_from_slices(&key, &key).unwrap());
            true
        }
    }

    fn flush_buffer(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        if self.cipher.is_some() {
            while self.pos != self.cap {
                self.pos += ready!(
                    Pin::new(&mut self.tgt).poll_write(cx, &self.buffer[self.pos..self.cap])
                )?;
            }
        }
        Poll::Ready(Ok(()))
    }
}

impl<W: AsyncWriteExt + Unpin> AsyncWrite for Encryptor<W> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, io::Error>> {
        if self.cipher.is_some() {
            ready!(self.flush_buffer(cx))?;
            self.pos = 0;
            let cap = max(buf.len(), BUFFER_SIZE);
            self.cap = cap;
            // extra buffer to avoid double &mut self
            let mut encrypted_buffer = [0; BUFFER_SIZE];
            encrypted_buffer[..cap].copy_from_slice(&buf[..cap]);
            self.cipher
                .as_mut()
                .unwrap()
                .encrypt(&mut encrypted_buffer[..cap]);
            self.buffer[..cap].copy_from_slice(&encrypted_buffer[..cap]);
            let _ = self.flush_buffer(cx)?;
            Poll::Ready(Ok(cap))
        } else {
            Pin::new(&mut self.tgt).poll_write(cx, buf)
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        self.flush_buffer(cx)
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), io::Error>> {
        ready!(self.flush_buffer(cx))?;
        Pin::new(&mut self.tgt).poll_shutdown(cx)
    }
}

pub struct Decryptor<R: AsyncReadExt + Unpin> {
    cipher: Option<Encryption>,
    src: R,
}

impl<R: AsyncReadExt + Unpin> Decryptor<R> {
    pub fn new(src: R) -> Decryptor<R> {
        Decryptor { src, cipher: None }
    }

    pub fn set_key(&mut self, key: [u8; 16]) -> bool {
        if self.cipher.is_some() {
            false
        } else {
            self.cipher = Some(Encryption::new_from_slices(&key, &key).unwrap());
            true
        }
    }
}

impl<R: AsyncReadExt + Unpin> AsyncRead for Decryptor<R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let prev_len = buf.filled().len();
        Pin::new(&mut self.src).poll_read(cx, buf).map_ok(|_| {
            if let Some(cipher) = &mut self.cipher {
                cipher.decrypt(&mut buf.filled_mut()[prev_len..]);
            }
        })
    }
}
