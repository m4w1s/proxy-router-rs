<div align="center">
    <h1>proxy-router</h1>
    <p>SOCKS5 proxy router library written in async rust</p>
</div>

## Features
- Can downstream requests to both http and socks5 proxies
- Cross-platform
- Built on-top of `tokio` library
- No **unsafe** code
- Ultra lightweight and scalable

## Install

```toml
proxy-router = { git = "https://github.com/m4w1s/proxy-router-rs", tag = "v0.1.0" }
```

## Usage

```rust
use proxy_router::router::socks5::{spawn_socks5_router, RouterOptions};
use proxy_router::proxy::Proxy;

async fn main() -> anyhow::Result<()> {
    let router_options = RouterOptions::builder()
        .proxy(Proxy::from_url("http://username:password@127.0.0.1:1234").unwrap())
        .listen_port(5000)
        .build()
        .unwrap();
    let join_handle = spawn_socks5_router(router_options).await?;

    println!("proxy router started on port 5000!");

    join_handle.await?;
}
```

## Inspired by

https://github.com/GlenDC/fast-socks5/tree/patch/server-router-support
