use async_http_proxy::{http_connect_tokio, http_connect_tokio_with_basic_auth, HttpError};
use derive_builder::Builder;
use fast_socks5::client::{Config as Socks5Config, Socks5Stream};
use fast_socks5::SocksError;
use std::time::Duration;
use thiserror::Error;
use tokio::net::TcpStream;
use url::{ParseError, Url};

#[derive(Debug, Clone, Default, PartialEq, Builder)]
pub struct Proxy {
    #[builder(default)]
    protocol: ProxyProtocol,
    #[builder(setter(into))]
    host: String,
    port: u16,
    #[builder(default)]
    auth: ProxyAuth,
}

impl Proxy {
    pub fn new(
        protocol: ProxyProtocol,
        host: impl Into<String>,
        port: impl Into<u16>,
        auth: ProxyAuth,
    ) -> Self {
        Self {
            protocol,
            host: host.into(),
            port: port.into(),
            auth,
        }
    }

    pub fn builder() -> ProxyBuilder {
        ProxyBuilder::default()
    }

    pub fn from_url(url: &str) -> Result<Self, ProxyError> {
        let parsed_url = Url::parse(url)?;
        let protocol = match parsed_url.scheme() {
            "http" | "https" => ProxyProtocol::Http,
            "socks5" => ProxyProtocol::Socks5,
            protocol => return Err(ProxyError::InvalidProtocol(protocol.to_string())),
        };
        let host = match parsed_url.host_str() {
            Some(host) => host.to_string(),
            None => return Err(ProxyError::InvalidHost),
        };
        let port = parsed_url.port_or_known_default().unwrap_or(80);
        let mut auth = ProxyAuth::None;

        match (parsed_url.username(), parsed_url.password()) {
            (username, Some(password)) if !username.is_empty() => {
                auth = ProxyAuth::Basic(BasicAuth::new(username, password));
            }
            _ => (),
        }

        Ok(Self {
            protocol,
            host,
            port,
            auth,
        })
    }
}

impl Proxy {
    pub async fn connect(
        &self,
        target_host: &str,
        target_port: u16,
    ) -> Result<TcpStream, ProxyError> {
        let proxy_addr = format!("{}:{}", self.host, self.port.to_string());

        let stream = match self.protocol {
            ProxyProtocol::Http => {
                let mut stream = match TcpStream::connect(proxy_addr).await {
                    Ok(stream) => stream,
                    Err(err) => return Err(HttpError::IoError(err).into()),
                };

                match &self.auth {
                    ProxyAuth::None => {
                        http_connect_tokio(&mut stream, target_host, target_port).await?;
                    }
                    ProxyAuth::Basic(BasicAuth { username, password }) => {
                        http_connect_tokio_with_basic_auth(
                            &mut stream,
                            target_host,
                            target_port,
                            username,
                            password,
                        )
                        .await?;
                    }
                }

                stream
            }
            ProxyProtocol::Socks5 => match &self.auth {
                ProxyAuth::None => Socks5Stream::connect(
                    proxy_addr,
                    target_host.to_string(),
                    target_port,
                    Socks5Config::default(),
                )
                .await?
                .get_socket(),
                ProxyAuth::Basic(BasicAuth { username, password }) => {
                    Socks5Stream::connect_with_password(
                        proxy_addr,
                        target_host.to_string(),
                        target_port,
                        username.to_string(),
                        password.to_string(),
                        Socks5Config::default(),
                    )
                    .await?
                    .get_socket()
                }
            },
        };

        Ok(stream)
    }

    pub async fn connect_with_timeout(
        &self,
        target_host: &str,
        target_port: u16,
        timeout: Duration,
    ) -> Result<TcpStream, ProxyError> {
        tokio::time::timeout(timeout, self.connect(target_host, target_port))
            .await
            .unwrap_or_else(|_| Err(ProxyError::ConnectionTimeout))
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub enum ProxyProtocol {
    #[default]
    Http,
    Socks5,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub enum ProxyAuth {
    #[default]
    None,
    Basic(BasicAuth),
}

#[derive(Debug, Clone, Default, PartialEq, Builder)]
pub struct BasicAuth {
    #[builder(setter(into))]
    username: String,
    #[builder(setter(into))]
    password: String,
}

impl BasicAuth {
    pub fn new(username: impl Into<String>, password: impl Into<String>) -> Self {
        Self {
            username: username.into(),
            password: password.into(),
        }
    }

    pub fn builder() -> BasicAuthBuilder {
        BasicAuthBuilder::default()
    }
}

#[derive(Debug, Error)]
pub enum ProxyError {
    #[error("Url parse error: {0}")]
    UrlParseError(#[from] ParseError),
    #[error("Invalid proxy protocol: {0}")]
    InvalidProtocol(String),
    #[error("Invalid proxy host")]
    InvalidHost,

    #[error("Connection timeout")]
    ConnectionTimeout,
    #[error("Http proxy error: {0}")]
    HttpError(#[from] HttpError),
    #[error("Socks proxy error: {0}")]
    SocksError(#[from] SocksError),
}
