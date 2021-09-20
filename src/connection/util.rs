use std::{
    cmp::min,
    pin::Pin,
    task::{Context, Poll},
};
use tokio::io::{self, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};

pub struct Limit<S> {
    stream: S,
    limit: usize,
}

impl<S> Limit<S> {
    pub fn remaining(&self) -> usize {
        self.limit
    }
}

impl<R: AsyncReadExt + Unpin> Limit<R> {
    pub fn new_read(reader: R, limit: usize) -> Limit<R> {
        Limit {
            stream: reader,
            limit,
        }
    }
}

impl<R: AsyncReadExt + Unpin> AsyncRead for Limit<R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if self.limit == 0 {
            Poll::Ready(Ok(()))
        } else {
            let start_filled_len = buf.filled().len();
            Pin::new(&mut self.stream).poll_read(cx, buf).map_ok(|_| {
                self.limit -= buf.filled().len() - start_filled_len;
            })
        }
    }
}

impl<W: AsyncWriteExt + Unpin> Limit<W> {
    pub fn new_write(writer: W, limit: usize) -> Limit<W> {
        Limit {
            stream: writer,
            limit,
        }
    }
}

impl<'a, W: AsyncWriteExt + Unpin> AsyncWrite for Limit<W> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, io::Error>> {
        if self.limit == 0 {
            Poll::Ready(Ok(0))
        } else {
            let len = min(buf.len(), self.limit);
            Pin::new(&mut self.stream)
                .poll_write(cx, &buf[..len])
                .map_ok(|size| {
                    self.limit -= size;
                    size
                })
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        Pin::new(&mut self.stream).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), io::Error>> {
        Pin::new(&mut self.stream).poll_shutdown(cx)
    }
}
