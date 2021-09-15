use std::{
    pin::Pin,
    task::{Context, Poll},
};
use tokio::io::{self, AsyncRead, AsyncReadExt, ReadBuf};

pub struct Limit<'a, R: AsyncReadExt + Unpin> {
    reader: &'a mut R,
    limit: usize,
}

impl<'a, R: AsyncReadExt + Unpin> Limit<'a, R> {
    pub fn new(reader: &'a mut R, limit: usize) -> Limit<'a, R> {
        Limit { reader, limit }
    }
}

impl<'a, R: AsyncReadExt + Unpin> AsyncRead for Limit<'a, R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if self.limit == 0 {
            Poll::Ready(Ok(()))
        } else {
            let buf = &mut buf.take(self.limit);
            Pin::new(&mut self.reader).poll_read(cx, buf).map_ok(|_| {
                self.limit -= buf.filled().len();
            })
        }
    }
}
