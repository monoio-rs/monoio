---
title: Poll IO 支持
date: 2024-10-21 17:00:00
author: ihciah
---

Poll IO 是 Monoio 中提供的一种新的 IO Driver 相关能力，借助这个能力可以良好地兼容使用 poll 风格 IO 接口的组件。

默认情况下，例如 `TcpStream` 会实现 `AsyncReadRent`/`AsyncWriteRent` trait。这种 trait 要求读写时转移所有权，因为只有这样才能在使用 io-uring 作为 driver 时保证内存安全。无论运行环境是否支持 io-uring，以该方式编写的代码都可以在不做任何修改的情况下借助 Monoio 内部的 fallback 机制直接运行。由于这种 IO trait 是面向完成通知的，下文将实现该类 trait 的 IO 称之为 `CompIo`。

但是，仍有大量的组件使用当前主流的 poll 风格 IO 接口，包括 `tokio::io::AsyncRead`，`async_std::io::Read`，以及 hyper 等组件为了支持多种 runtime 抽象出的 Read/Write 接口。在这些不同的 poll 风格 IO 接口之间转换是非常容易的。由于这种 IO trait 是在就绪是同步尝试执行的，下文中称之为 `PollIo`。

为了支持这些组件，Monoio 提供了：

1. `IntoPollIo`/`IntoCompIo`：在 `PollIo` 和 `CompIo` 之间转换。
2. `TcpStreamPoll` 等结构，实现 `tokio::io::AsyncRead/AsyncWrite`。

## 使用例子

1. 首先请确保你的项目在引入 `monoio` 时开启了 `poll-io` feature（需要 0.2.2 及以上版本）：

    ```toml
    [dependencies]
    monoio = { version = "0.3.0", features = ["poll-io"] }
    ```

2. 引入并使用 `IntoPollIo` 来将 `CompIo` 转换为 `PollIo`。
    在下面的例子中，我们将启动一个 Listener，并将 accept 到的连接转换为 PollIo，之后使用 tokio 的 `AsyncRead`/`AsyncWrite` trait 进行读写。

    ```rust
    use monoio::{
        io::{
            // Here AsyncReadExt and AsyncWriteExt are re-exported from and equivalent with tokio
            poll_io::{AsyncReadExt, AsyncWriteExt},
            IntoPollIo,
        },
        net::{TcpListener, TcpStream},
    };

    #[monoio::main(driver = "fusion")]
    async fn main() {
        let listener = TcpListener::bind("127.0.0.1:50002").unwrap();
        println!("listening");
        loop {
            let incoming = listener.accept().await;
            match incoming {
                Ok((stream, addr)) => {
                    println!("accepted a connection from {addr}");
                    monoio::spawn(echo(stream));
                }
                Err(e) => {
                    println!("accepted connection failed: {e}");
                    return;
                }
            }
        }
    }

    async fn echo(stream: TcpStream) -> std::io::Result<()> {
        // Convert completion-based io to poll-based io(which impl tokio::io)
        let mut stream = stream.into_poll_io()?;
        let mut buf: Vec<u8> = vec![0; 1024];
        let mut res;
        loop {
            // read
            res = stream.read(&mut buf).await?;
            if res == 0 {
                return Ok(());
            }

            // write all
            stream.write_all(&buf[0..res]).await?;
        }
    }
    ```

    更多例子可以参考 [examples/h2_client.rs](../../examples/h2_client.rs) 和 [examples/h2_server.rs](../../examples/h2_server.rs)。

## 底层实现

Poll IO 能力基于 epoll over io-uring 实现：

1. 如果当前 driver 为 Legacy Driver（例如，在 macOS，或低版本 Linux 平台上），那么可以直接对外提供 Poll IO 能力；
2. 如果当前 driver 为 io-uring Driver，那么开启本 feature 后，会在 io-uring 上添加一个 epoll fd，并基于这个 epoll fd 对外提供 Poll IO 能力。

当我们将一个 `CompIo` 转换为 `PollIo` 时，如果当前运行在 io-uring 上，则需要取得在其中维护的 epoll fd 并向其注册该 `CompIo` 对应的文件描述符。这样，当 `CompIo` 就绪时，Runtime 就可以通过 io-uring 的完成通知感知到 epoll fd 就绪，并执行 `epoll_wait(0)` 以获得所有在其上的就绪事件。

如果当前 driver 非 io-uring，则转换操作是几乎没有成本的。

在 Linux 平台上，IO trait 与底层 driver 对应关系：

1. `CompIo` -> `io-uring` / `epoll`
2. `PollIo` -> `epoll` / `epoll over io-uring`

## 注意事项

1. 使用该功能可以良好地支持 hyper、h2 等库；但为了充分利用 io-uring 的能力，建议在 hot path 上使用 `CompIo` 风格的 IO 接口（`AsyncReadRent`、`AsyncWriteRent`）；
2. 该功能已经过较大规模的生产环境验证，但当前可能有部分组件没有支持 `IntoPollIo`/`IntoCompIo` 转换能力，欢迎汇报相关问题；
