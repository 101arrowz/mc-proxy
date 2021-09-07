#![feature(type_alias_impl_trait)]
#![feature(generic_associated_types)]
#![feature(associated_type_defaults)]

use std::future::Future;
use std::iter::once;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{self, AsyncRead, AsyncReadExt, AsyncWriteExt, Error, ErrorKind, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::{sleep, Duration, Instant, Sleep};
use trust_dns_resolver::config::{ResolverConfig, ResolverOpts};
use trust_dns_resolver::AsyncResolver;

mod protocol;

struct Lag<R: AsyncRead> {
    delay: Duration,
    sleep: Pin<Box<Sleep>>,
    sleep_done: bool,
    inner: R,
}

impl<R: AsyncRead> Lag<R> {
    fn new(inner: R, delay: Duration) -> Lag<R> {
        Self {
            inner,
            delay,
            sleep: Box::pin(sleep(delay)),
            sleep_done: false,
        }
    }
}

impl<R: AsyncRead + Unpin> AsyncRead for Lag<R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context,
        buf: &mut ReadBuf,
    ) -> Poll<io::Result<()>> {
        match self.sleep.as_mut().poll(cx) {
            Poll::Ready(_) => {
                let delay = self.delay;
                self.sleep.as_mut().reset(Instant::now() + delay);
                Pin::new(&mut self.inner).poll_read(cx, buf)
            }
            Poll::Pending => Poll::Pending,
        }
        // if !self.sleep_done {
        //     if self.sleep.as_mut().poll(cx) == Poll::Pending {
        //         return Poll::Pending;
        //     }
        //     self.sleep_done = true;
        // }
        // match Pin::new(&mut self.inner).poll_read(cx, buf) {
        //     Poll::Pending => Poll::Pending,
        //     Poll::Ready(result) => {
        //         let new_deadline = Instant::now() + self.delay;
        //         self.sleep_done = false;
        //         self.sleep.as_mut().reset(new_deadline);
        //         Poll::Ready(result)
        //     }
        // }
    }
}

async fn read_varint<R: AsyncReadExt + Unpin>(
    reader: &mut R,
) -> Result<i32, Box<dyn std::error::Error>> {
    let mut bit_offset = 0;
    let mut buf = [0; 1];
    let mut result = 0;
    loop {
        if bit_offset == 35 || reader.read(&mut buf).await? == 0 {
            return Err(Box::new(Error::from(ErrorKind::UnexpectedEof)));
        }
        let val = buf[0] as i32;
        result |= (val & 127) << bit_offset;
        if val >> 7 == 0 {
            break;
        }
        bit_offset += 7;
    }
    Ok(result)
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let target = "lunar.gg";
    let resolver = AsyncResolver::tokio(ResolverConfig::default(), ResolverOpts::default())?;
    let (remote_host, remote_port) = if let Ok(srv) = resolver
        .srv_lookup(["_minecraft._tcp.", target].concat())
        .await
    {
        let record = srv.iter().next().unwrap();
        let host = record.target().to_string();
        (host, record.port())
    } else {
        (target.to_string(), 25565)
    };
    let ip_addr = resolver
        .lookup_ip(remote_host.to_string())
        .await?
        .iter()
        .next()
        .unwrap();
    let delay = Duration::from_millis(50);
    let local_server = TcpListener::bind("localhost:25565").await?;
    loop {
        let (mut client, addr) = local_server.accept().await?;
        let remote_host = remote_host.clone();
        tokio::spawn(async move {
            let (client_out, mut client_in) = client.split();
            let mut client_out = Lag::new(client_out, delay);
            if let Ok(mut server) = TcpStream::connect((ip_addr, remote_port)).await {
                let (server_out, mut server_in) = server.split();
                let mut server_out = Lag::new(server_out, delay);
                let (_a, _b) = tokio::join!(io::copy(&mut server_out, &mut client_in), async {
                    let mut bad = [0u8; 16];
                    client_out.read(&mut bad).await?;
                    let mut first_msg: Vec<u8> = vec![
                        6 + remote_host.len() as u8,
                        0,
                        47,
                        // server name here
                        (remote_port >> 8) as u8,
                        remote_port as u8,
                        bad[15],
                    ];
                    first_msg.splice(
                        3..3,
                        once(remote_host.len() as u8).chain(remote_host.as_bytes().iter().cloned()),
                    );
                    server_in.write_all(&first_msg).await.unwrap();
                    io::copy(&mut client_out, &mut server_in).await
                });
            }
        });
    }
}
