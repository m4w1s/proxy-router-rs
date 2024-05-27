use crate::proxy::{Proxy, ProxyError};
use anyhow::Context;
use async_http_proxy::HttpError;
use derive_builder::Builder;
use fast_socks5::server::{Config as Socks5Config, DenyAuthentication, Socks5Server, Socks5Socket};
use fast_socks5::{ReplyError, Socks5Command, SocksError};
use log::{debug, error, info};
use std::io::ErrorKind;
use std::net::ToSocketAddrs;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio::task;
use tokio_stream::StreamExt;

#[derive(Debug, Clone, Default, PartialEq, Builder)]
#[builder(setter(strip_option))]
pub struct RouterOptions {
    proxy: Proxy,
    listen_port: u16,
    #[builder(setter(into), default)]
    listen_host: Option<String>,
    #[builder(default = "Duration::from_secs(10)")]
    request_timeout: Duration,
}

impl RouterOptions {
    pub fn builder() -> RouterOptionsBuilder {
        RouterOptionsBuilder::default()
    }
}

pub async fn spawn_socks5_router(options: RouterOptions) -> anyhow::Result<task::JoinHandle<()>> {
    let listen_addr = [
        options.listen_host.unwrap_or("127.0.0.1".to_string()),
        options.listen_port.to_string(),
    ]
    .join(":");
    let mut server_config = <Socks5Config>::default();

    server_config.set_execute_command(false);
    server_config.set_request_timeout(options.request_timeout.as_secs());

    let listener = <Socks5Server>::bind(&listen_addr)
        .await
        .context(format!("Can't bind the socks5 server to {}", listen_addr))?
        .with_config(server_config);

    let join_handle = task::spawn(async move {
        let mut incoming = listener.incoming();

        while let Some(socket_res) = incoming.next().await {
            match socket_res {
                Ok(socket) => {
                    let proxy = options.proxy.clone();

                    task::spawn(async move {
                        if let Err(err) =
                            handle_socket(socket, proxy, options.request_timeout).await
                        {
                            error!("Socket handle error: {:#}", err);
                        }
                    });
                }
                Err(err) => {
                    error!("Socket accept error: {:#}", err);
                }
            }
        }
    });

    Ok(join_handle)
}

async fn handle_socket(
    socket: Socks5Socket<TcpStream, DenyAuthentication>,
    proxy: Proxy,
    timeout: Duration,
) -> Result<(), SocksError> {
    let mut socks5_socket = socket.upgrade_to_socks5().await?;

    match execute_command(&mut socks5_socket, proxy, timeout).await {
        Ok(_) => (),
        Err(SocksError::ReplyError(err)) => {
            // If a reply error has been returned, we send it to the client
            socks5_socket.reply_error(&err).await?;

            return Err(err.into());
        }
        Err(err) => return Err(err),
    }

    Ok(())
}

async fn execute_command(
    socket: &mut Socks5Socket<TcpStream, DenyAuthentication>,
    proxy: Proxy,
    timeout: Duration,
) -> Result<(), SocksError> {
    match socket.get_command() {
        None => Err(ReplyError::CommandNotSupported.into()),
        Some(cmd) => match cmd {
            Socks5Command::TCPBind => Err(ReplyError::CommandNotSupported.into()),
            Socks5Command::TCPConnect => execute_command_connect(socket, proxy, timeout).await,
            Socks5Command::UDPAssociate => Err(ReplyError::CommandNotSupported.into()),
        },
    }
}

async fn execute_command_connect(
    socket: &mut Socks5Socket<TcpStream, DenyAuthentication>,
    proxy: Proxy,
    timeout: Duration,
) -> Result<(), SocksError> {
    let socket_addr = socket
        .target_addr()
        .context("Empty target address")?
        .to_socket_addrs()?
        .next()
        .context("Unreachable target")?;

    let mut downstream = match proxy
        .connect_with_timeout(&socket_addr.ip().to_string(), socket_addr.port(), timeout)
        .await
    {
        Ok(stream) => stream,
        Err(err) => return Err(map_proxy_connect_error(err).into()),
    };

    debug!("Connected to downstream proxy");

    socket
        .write(&vec![
            5, // protocol version = socks5
            0, // reply code = succeeded
            0, // reserved
            1, // address type = ipv4
            127, 0, 0, 1, // address = 127.0.0.1
            0, 0, // port = 0
        ])
        .await
        .context("Can't write successful reply")?;
    socket.flush().await.context("Can't flush the reply")?;

    debug!("Start data transfer");

    match tokio::io::copy_bidirectional(&mut downstream, socket).await {
        Ok(res) => {
            info!("Socket transfer finished ({}, {})", res.0, res.1);
        }
        Err(err) => match err.kind() {
            ErrorKind::NotConnected => {
                info!("Socket transfer closed by client");
            }
            ErrorKind::ConnectionReset => {
                info!("Socket transfer closed by downstream proxy");
            }
            _ => return Err(err.into()),
        },
    }

    Ok(())
}

fn map_proxy_connect_error(err: ProxyError) -> ReplyError {
    let mut io_error: Option<std::io::Error> = None;

    match err {
        ProxyError::ConnectionTimeout => return ReplyError::ConnectionTimeout,
        ProxyError::HttpError(HttpError::IoError(err)) => {
            io_error = Some(err);
        }
        ProxyError::SocksError(SocksError::Io(err)) => {
            io_error = Some(err);
        }
        _ => (),
    };

    if io_error.is_some() {
        match io_error.unwrap().kind() {
            ErrorKind::ConnectionRefused => {
                return ReplyError::ConnectionRefused;
            }
            ErrorKind::ConnectionAborted => {
                return ReplyError::ConnectionNotAllowed;
            }
            ErrorKind::ConnectionReset => {
                return ReplyError::ConnectionNotAllowed;
            }
            ErrorKind::NotConnected => return ReplyError::NetworkUnreachable,
            _ => (),
        }
    }

    return ReplyError::GeneralFailure;
}
