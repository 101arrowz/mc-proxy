use aes::Aes128;
use cfb8::{Cfb8, cipher::{NewCipher, AsyncStreamCipher, errors::InvalidLength}};
use tokio::io::{self, AsyncRead, AsyncReadExt, AsyncWrite, ReadBuf};
use std::{pin::Pin, task::{Context, Poll, ready}, cmp::max};

type MCEncryption = Cfb8<Aes128>;

pub struct MCEncryptor<W: AsyncWrite + Unpin> {
    cipher: Option<MCEncryption>,
    tgt: W,
    buffer: Box<[u8]>,
    pos: usize,
    cap: usize
}

impl<W: AsyncWrite + Unpin> MCEncryptor<W> {
    pub fn new(tgt: W) -> MCEncryptor<W> {
        MCEncryptor {
            cipher: None,
            tgt,
            buffer: vec![0u8; 8192].into_boxed_slice(),
            pos: 0,
            cap: 0
        }
    }

    pub fn set_key(&mut self, key: [u8; 16]) {
        self.cipher = Some(MCEncryption::new_from_slices(&key, &key).unwrap());
    }

    fn flush_buffer(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        if self.cipher.is_some() {
            while self.pos != self.cap {
                self.pos += ready!(Pin::new(&mut self.tgt).poll_write(cx, &self.buffer[self.pos..self.cap]))?;
            }
        }
        Poll::Ready(Ok(()))
    }
}

impl<W: AsyncWrite + Unpin> AsyncWrite for MCEncryptor<W> {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<Result<usize, io::Error>> {
        if let Some(cipher) = self.cipher {
            ready!(self.flush_buffer(cx))?;
            self.pos = 0;
            let cap = max(buf.len(), 8192);
            self.cap = cap;
            // Need intermediate to avoid double borrow...
            let mut encrypted_buffer = [0; 8192];
            encrypted_buffer[..cap].copy_from_slice(&buf[..cap]);
            cipher.encrypt(&mut encrypted_buffer[..cap]);
            self.buffer[..cap].copy_from_slice(&encrypted_buffer[..cap]);
            ready!(self.flush_buffer(cx))?;
            Poll::Ready(Ok(cap))
        } else {
            Pin::new(&mut self.tgt).poll_write(cx, buf)
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        self.flush_buffer(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        ready!(self.flush_buffer(cx))?;
        Pin::new(&mut self.tgt).poll_shutdown(cx)
    }
}

pub struct MCDecryptor<R: AsyncReadExt + Unpin> {
    cipher: Option<MCEncryption>,
    src: R,
}

impl<R: AsyncReadExt + Unpin> MCDecryptor<R> {
    pub fn new(src: R) -> MCDecryptor<R> {
        MCDecryptor {
            src,
            cipher: None
        }
    }

    pub fn set_key(&mut self, key: [u8; 16]) {
        self.cipher = Some(MCEncryption::new_from_slices(&key, &key).unwrap());
    }
}

impl<R: AsyncReadExt + Unpin> AsyncRead for MCDecryptor<R> {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        let prev_len = buf.filled().len();
        Pin::new(&mut self.src).poll_read(cx, buf).map_ok(|_| {
            if let Some(cipher) = self.cipher {
                cipher.decrypt(&mut buf.filled_mut()[prev_len..]);
            }
        })
    }
}