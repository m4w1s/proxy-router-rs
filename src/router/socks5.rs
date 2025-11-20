use crate::proxy::Proxy;
use derive_builder::Builder;
use fast_socks5::server::{Socks5ServerProtocol, transfer};
use fast_socks5::{ReplyError, Socks5Command, SocksError};
use log::{error, info};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;
use tokio::net::{TcpListener, TcpStream};
use tokio::task;

#[derive(Debug, Clone, Default, PartialEq, Builder)]
#[builder(setter(strip_option))]
pub struct RouterOptions {
    proxy: Proxy,
    listen_port: u16,
    #[builder(setter(into), default)]
    listen_host: Option<String>,
    #[builder(default = "Duration::from_secs(10)")]
    timeout: Duration,
}

impl RouterOptions {
    pub fn builder() -> RouterOptionsBuilder {
        RouterOptionsBuilder::default()
    }
}

pub async fn spawn_socks5_router(options: RouterOptions) -> std::io::Result<task::JoinHandle<()>> {
    let listen_addr = [
        options
            .listen_host
            .unwrap_or_else(|| "127.0.0.1".to_string()),
        options.listen_port.to_string(),
    ]
    .join(":");
    let listener = TcpListener::bind(&listen_addr).await?;

    info!("Listen for socks connections @ {listen_addr}");

    let join_handle = task::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((socket, _)) => {
                    let proxy = options.proxy.clone();

                    task::spawn(async move {
                        if let Err(err) = on_connect(socket, proxy, options.timeout).await {
                            error!("Socks connection handle error: {err}");
                        }
                    });
                }
                Err(err) => {
                    error!("Socks connection accept error: {err}");
                }
            }
        }
    });

    Ok(join_handle)
}

async fn on_connect(socket: TcpStream, proxy: Proxy, timeout: Duration) -> Result<(), SocksError> {
    let (proto, cmd, target_addr) = Socks5ServerProtocol::accept_no_auth(socket)
        .await?
        .read_command()
        .await?;

    if cmd != Socks5Command::TCPConnect {
        let err = ReplyError::CommandNotSupported;

        proto.reply_error(&err).await?;
        return Err(err.into());
    }

    let (target_host, target_port) = target_addr.into_string_and_port();
    let proxy_socket = match proxy
        .connect_with_timeout(target_host, target_port, timeout)
        .await
    {
        Ok(stream) => stream,
        Err(_) => {
            let err = ReplyError::NetworkUnreachable;

            proto.reply_error(&err).await?;
            return Err(err.into());
        }
    };
    let inner_socket = proto
        .reply_success(SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 0))
        .await?;

    transfer(inner_socket, proxy_socket).await;

    Ok(())
}
