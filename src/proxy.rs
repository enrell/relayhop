//! A bounded SOCKS5 CONNECT server. TLS stays end-to-end with Discord.
use std::{
    net::{IpAddr, SocketAddr},
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, Result, bail, ensure};
use futures::future::BoxFuture;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{Notify, Semaphore},
    task::JoinSet,
    time::timeout,
};
use tokio_util::sync::CancellationToken;

pub trait IoStream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> IoStream for T {}
pub type Stream = Box<dyn IoStream>;

pub trait Connector: Send + Sync {
    fn connect<'a>(&'a self, target: &'a Target) -> BoxFuture<'a, Result<Stream>>;
}

#[derive(Clone, Debug)]
pub struct Target {
    pub host: String,
    pub port: u16,
}

impl Target {
    pub fn new(host: &str, port: u16) -> Result<Self> {
        let host = host.strip_suffix('.').unwrap_or(host).to_ascii_lowercase();
        ensure!(port == 443, "only HTTPS destinations are allowed");
        ensure!(!host.is_empty() && host.len() <= 253, "invalid hostname");
        ensure!(
            host.split('.').all(|label| !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'-')),
            "invalid hostname"
        );
        let allowed = [
            "discord.com",
            "discord.gg",
            "discordapp.com",
            "discordapp.net",
            "discord.media",
            "discordcdn.com",
            "discordstatus.com",
        ];
        ensure!(
            allowed
                .iter()
                .any(|suffix| host == *suffix || host.ends_with(&format!(".{suffix}"))),
            "destination is not a Discord domain"
        );
        Ok(Self { host, port })
    }
}

pub struct Direct;
impl Connector for Direct {
    fn connect<'a>(&'a self, target: &'a Target) -> BoxFuture<'a, Result<Stream>> {
        Box::pin(async move {
            let addresses: Vec<_> = tokio::net::lookup_host((target.host.as_str(), target.port))
                .await?
                .collect();
            ensure!(!addresses.is_empty(), "DNS returned no addresses");
            // Reject the entire DNS answer if it contains local/reserved addresses.
            ensure!(
                addresses.iter().all(|address| public_address(address.ip())),
                "DNS returned a non-public address"
            );
            let mut last = None;
            for address in addresses {
                match timeout(Duration::from_secs(5), TcpStream::connect(address)).await {
                    Ok(Ok(stream)) => return Ok(Box::new(stream) as Stream),
                    Ok(Err(error)) => last = Some(error.to_string()),
                    Err(_) => last = Some("connection timed out".into()),
                }
            }
            bail!("direct connection failed: {}", last.unwrap_or_default())
        })
    }
}

fn public_address(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let [a, b, c, _] = ip.octets();
            !(ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_broadcast()
                || ip.is_documentation()
                || a == 0
                || a >= 224
                || (a == 100 && (64..=127).contains(&b))
                || (a == 198 && (b == 18 || b == 19))
                || (a == 192 && b == 0 && c == 0))
        }
        IpAddr::V6(ip) => {
            let s = ip.segments();
            // Public global unicast only; exclude documentation and transition ranges.
            (s[0] & 0xe000) == 0x2000
                && s[0] != 0x2002
                && !(s[0] == 0x2001 && (s[1] < 0x200 || s[1] == 0xdb8))
                && !(s[0] == 0x3fff && s[1] < 0x1000)
        }
    }
}

pub struct SocksUpstream(pub SocketAddr);
impl Connector for SocksUpstream {
    fn connect<'a>(&'a self, target: &'a Target) -> BoxFuture<'a, Result<Stream>> {
        Box::pin(async move { Ok(Box::new(socks_connect(self.0, target).await?) as Stream) })
    }
}

pub async fn socks_connect(address: SocketAddr, target: &Target) -> Result<TcpStream> {
    let mut stream = TcpStream::connect(address).await?;
    stream.write_all(&[5, 1, 0]).await?;
    let mut greeting = [0; 2];
    stream.read_exact(&mut greeting).await?;
    ensure!(greeting == [5, 0], "SOCKS authentication rejected");
    let mut request = vec![5, 1, 0, 3, target.host.len() as u8];
    request.extend_from_slice(target.host.as_bytes());
    request.extend_from_slice(&target.port.to_be_bytes());
    stream.write_all(&request).await?;
    let mut header = [0; 4];
    stream.read_exact(&mut header).await?;
    ensure!(
        header[0] == 5 && header[1] == 0 && header[2] == 0,
        "SOCKS connection rejected ({})",
        header[1]
    );
    let count = match header[3] {
        1 => 4,
        4 => 16,
        3 => stream.read_u8().await? as usize,
        _ => bail!("invalid SOCKS reply"),
    };
    let mut rest = vec![0; count + 2];
    stream.read_exact(&mut rest).await?;
    Ok(stream)
}

async fn reply(stream: &mut TcpStream, code: u8) -> Result<()> {
    stream.write_all(&[5, code, 0, 1, 0, 0, 0, 0, 0, 0]).await?;
    Ok(())
}

async fn request(stream: &mut TcpStream) -> Result<Target> {
    ensure!(stream.read_u8().await? == 5, "unsupported SOCKS version");
    let count = stream.read_u8().await?;
    let mut methods = vec![0; count as usize];
    stream.read_exact(&mut methods).await?;
    if !methods.contains(&0) {
        stream.write_all(&[5, 255]).await?;
        bail!("no supported authentication method");
    }
    stream.write_all(&[5, 0]).await?;
    let mut header = [0; 4];
    stream.read_exact(&mut header).await?;
    if header[0] != 5 || header[2] != 0 {
        reply(stream, 1).await?;
        bail!("malformed SOCKS request");
    }
    if header[1] != 1 {
        reply(stream, 7).await?;
        bail!("only CONNECT is supported");
    }
    if header[3] != 3 {
        reply(stream, 8).await?;
        bail!("a DNS hostname is required");
    }
    let length = stream.read_u8().await?;
    let mut name = vec![0; length as usize];
    stream.read_exact(&mut name).await?;
    let port = stream.read_u16().await?;
    match std::str::from_utf8(&name)
        .context("invalid hostname")
        .and_then(|name| Target::new(name, port))
    {
        Ok(target) => Ok(target),
        Err(error) => {
            reply(stream, 2).await?;
            Err(error)
        }
    }
}

pub struct Router {
    pub initial: Arc<dyn Connector>,
    pub direct: Arc<dyn Connector>,
    pub switched: CancellationToken,
    pub first_connection: Notify,
}

impl Router {
    pub fn new(initial: Arc<dyn Connector>, direct: Arc<dyn Connector>) -> Arc<Self> {
        Arc::new(Self {
            initial,
            direct,
            switched: CancellationToken::new(),
            first_connection: Notify::new(),
        })
    }

    pub fn switch_direct(&self) {
        // Cancels both established Tor tunnels and connections still being opened.
        // Chromium reconnects; future connections enter the direct branch.
        self.switched.cancel();
    }

    async fn handle(&self, mut downstream: TcpStream) -> Result<()> {
        let target = timeout(Duration::from_secs(10), request(&mut downstream)).await??;
        if self.switched.is_cancelled() {
            return self
                .tunnel(&mut downstream, &target, self.direct.as_ref())
                .await;
        }
        tokio::select! {
            biased;
            _ = self.switched.cancelled() => Ok(()),
            result = self.tunnel(&mut downstream, &target, self.initial.as_ref()) => result,
        }
    }

    async fn tunnel(
        &self,
        downstream: &mut TcpStream,
        target: &Target,
        connector: &dyn Connector,
    ) -> Result<()> {
        let mut upstream = match timeout(Duration::from_secs(35), connector.connect(target)).await {
            Ok(Ok(stream)) => stream,
            result => {
                reply(downstream, 4).await?;
                return match result {
                    Ok(Err(error)) => Err(error),
                    _ => Err(anyhow::anyhow!("upstream timed out")),
                };
            }
        };
        reply(downstream, 0).await?;
        self.first_connection.notify_one();
        tokio::io::copy_bidirectional(downstream, &mut upstream).await?;
        Ok(())
    }
}

pub async fn serve(
    listener: TcpListener,
    router: Arc<Router>,
    stop: CancellationToken,
) -> Result<()> {
    ensure!(
        listener.local_addr()?.ip().is_loopback(),
        "proxy must listen on loopback"
    );
    let permits = Arc::new(Semaphore::new(128));
    let mut tasks = JoinSet::new();
    loop {
        tokio::select! {
            biased;
            _ = stop.cancelled() => break,
            Some(_) = tasks.join_next(), if !tasks.is_empty() => {},
            incoming = listener.accept() => {
                let (stream, peer) = incoming?;
                if !peer.ip().is_loopback() { continue; }
                let Ok(permit) = permits.clone().try_acquire_owned() else { continue; };
                let router = router.clone();
                tasks.spawn(async move {
                    let _permit = permit;
                    if let Err(error) = router.handle(stream).await {
                        tracing::debug!(%error, "proxy connection ended");
                    }
                });
            }
        }
    }
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Echo(Arc<AtomicUsize>);
    impl Connector for Echo {
        fn connect<'a>(&'a self, _: &'a Target) -> BoxFuture<'a, Result<Stream>> {
            Box::pin(async move {
                self.0.fetch_add(1, Ordering::SeqCst);
                let (a, mut b) = tokio::io::duplex(4096);
                tokio::spawn(async move {
                    let (mut read, mut write) = tokio::io::split(&mut b);
                    let _ = tokio::io::copy(&mut read, &mut write).await;
                });
                Ok(Box::new(a) as Stream)
            })
        }
    }

    #[test]
    fn domain_boundary_and_port_policy() {
        for name in ["discord.com", "Gateway.Discord.GG.", "cdn.discordapp.com"] {
            assert!(Target::new(name, 443).is_ok());
        }
        for name in [
            "evil-discord.com",
            "discord.com.evil.test",
            "127.0.0.1",
            "localhost",
            "x..discord.com",
            "x/discord.com",
            "x\n.discord.com",
            "-x.discord.com",
            "discord.com..",
        ] {
            assert!(Target::new(name, 443).is_err(), "{name}");
        }
        assert!(Target::new("discord.com", 80).is_err());
    }

    #[test]
    fn dns_cannot_redirect_to_local_network() {
        for ip in [
            "127.0.0.1",
            "10.0.0.1",
            "169.254.169.254",
            "100.64.0.1",
            "192.168.1.1",
            "198.18.0.1",
            "224.0.0.1",
            "::1",
            "::ffff:127.0.0.1",
            "fd00::1",
            "2001:db8::1",
        ] {
            assert!(!public_address(ip.parse().unwrap()), "{ip}");
        }
        assert!(public_address("1.1.1.1".parse().unwrap()));
        assert!(public_address("2606:4700:4700::1111".parse().unwrap()));
    }

    #[tokio::test]
    async fn switch_closes_old_tunnel_and_new_connections_are_direct() -> Result<()> {
        let initial_count = Arc::new(AtomicUsize::new(0));
        let direct_count = Arc::new(AtomicUsize::new(0));
        let router = Router::new(
            Arc::new(Echo(initial_count.clone())),
            Arc::new(Echo(direct_count.clone())),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let stop = CancellationToken::new();
        let task = tokio::spawn(serve(listener, router.clone(), stop.clone()));
        let target = Target::new("discord.com", 443)?;
        let mut first = socks_connect(address, &target).await?;
        first.write_all(b"opaque TLS bytes").await?;
        let mut data = [0; 16];
        first.read_exact(&mut data).await?;
        assert_eq!(&data, b"opaque TLS bytes");
        router.switch_direct();
        assert_eq!(
            timeout(Duration::from_secs(2), first.read(&mut data)).await??,
            0
        );
        let mut second = socks_connect(address, &target).await?;
        second.write_all(b"ok").await?;
        second.read_exact(&mut data[..2]).await?;
        assert_eq!(&data[..2], b"ok");
        assert_eq!(initial_count.load(Ordering::SeqCst), 1);
        assert_eq!(direct_count.load(Ordering::SeqCst), 1);
        stop.cancel();
        task.await??;
        Ok(())
    }

    #[tokio::test]
    async fn forbidden_destination_gets_socks_failure() -> Result<()> {
        let count = Arc::new(AtomicUsize::new(0));
        let connector = Arc::new(Echo(count.clone()));
        let router = Router::new(connector.clone(), connector);
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let stop = CancellationToken::new();
        let task = tokio::spawn(serve(listener, router, stop.clone()));
        let target = Target {
            host: "attacker.example".into(),
            port: 443,
        };
        assert!(socks_connect(address, &target).await.is_err());
        assert_eq!(count.load(Ordering::SeqCst), 0);
        stop.cancel();
        task.await??;
        Ok(())
    }

    struct Pending {
        started: Notify,
        dropped: Arc<AtomicUsize>,
    }

    impl Connector for Pending {
        fn connect<'a>(&'a self, _: &'a Target) -> BoxFuture<'a, Result<Stream>> {
            Box::pin(async move {
                struct Guard(Arc<AtomicUsize>);
                impl Drop for Guard {
                    fn drop(&mut self) {
                        self.0.fetch_add(1, Ordering::SeqCst);
                    }
                }
                let _guard = Guard(self.dropped.clone());
                self.started.notify_one();
                futures::future::pending().await
            })
        }
    }

    #[tokio::test]
    async fn switch_cancels_inflight_tor_connect_without_waiting_for_timeout() -> Result<()> {
        let dropped = Arc::new(AtomicUsize::new(0));
        let pending = Arc::new(Pending {
            started: Notify::new(),
            dropped: dropped.clone(),
        });
        let direct_count = Arc::new(AtomicUsize::new(0));
        let router = Router::new(pending.clone(), Arc::new(Echo(direct_count.clone())));
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let stop = CancellationToken::new();
        let server = tokio::spawn(serve(listener, router.clone(), stop.clone()));
        let first =
            tokio::spawn(
                async move { socks_connect(address, &Target::new("discord.com", 443)?).await },
            );
        timeout(Duration::from_secs(2), pending.started.notified()).await?;
        router.switch_direct();
        assert!(timeout(Duration::from_secs(2), first).await??.is_err());
        assert_eq!(dropped.load(Ordering::SeqCst), 1);
        let _direct = socks_connect(address, &Target::new("discord.com", 443)?).await?;
        assert_eq!(direct_count.load(Ordering::SeqCst), 1);
        stop.cancel();
        server.await??;
        Ok(())
    }

    #[tokio::test]
    async fn malformed_or_unsupported_requests_never_reach_upstream() -> Result<()> {
        let count = Arc::new(AtomicUsize::new(0));
        let connector = Arc::new(Echo(count.clone()));
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let stop = CancellationToken::new();
        let server = tokio::spawn(serve(
            listener,
            Router::new(connector.clone(), connector),
            stop.clone(),
        ));
        for (header, expected) in [([5, 3, 0, 3], 7), ([5, 1, 0, 1], 8), ([5, 1, 1, 3], 1)] {
            let mut client = TcpStream::connect(address).await?;
            // Fragmentation is legal: the server must use read_exact semantics.
            for byte in [5, 1, 0] {
                client.write_all(&[byte]).await?;
            }
            let mut greeting = [0; 2];
            client.read_exact(&mut greeting).await?;
            assert_eq!(greeting, [5, 0]);
            client.write_all(&header).await?;
            let mut response = [0; 10];
            client.read_exact(&mut response).await?;
            assert_eq!(response[1], expected);
        }
        let mut client = TcpStream::connect(address).await?;
        client.write_all(&[5, 1, 2]).await?; // only username/password offered
        let mut response = [0; 2];
        client.read_exact(&mut response).await?;
        assert_eq!(response, [5, 255]);
        assert_eq!(count.load(Ordering::SeqCst), 0);
        stop.cancel();
        server.await??;
        Ok(())
    }
}
