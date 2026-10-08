---
title: Poll IO Support
date: 2024-10-21 17:00:00
author: ihciah
---

Poll IO is a new IO Driver capability provided in Monoio, which allows for good compatibility with components that use poll-style IO interfaces.

By default, for example, `TcpStream` will implement the `AsyncReadRent`/`AsyncWriteRent` traits. These traits require ownership transfer during read and write operations to ensure memory safety when using io-uring as the driver. Code written this way can run directly without modification, leveraging Monoio’s internal fallback mechanism, regardless of whether the runtime environment supports io-uring or not. Since this IO trait is completion-based, we refer to IOs implementing this type of trait as `CompIo` in the following text.

However, many components still use mainstream poll-style IO interfaces, including `tokio::io::AsyncRead`, `async_std::io::Read`, and components like hyper, which abstract Read/Write interfaces to support multiple runtimes. Converting between these different poll-style IO interfaces is relatively easy. Since this IO trait is a synchronous attempt executed when ready, we will refer to it as `PollIo` in the following text.

To support these components, Monoio provides:

1. `IntoPollIo`/`IntoCompIo`: for converting between `PollIo` and `CompIo`.
2. Structures like `TcpStreamPoll`, which implement `tokio::io::AsyncRead/AsyncWrite`.

## Usage Example

1. First, ensure that your project enables the `poll-io` feature when adding `monoio` as a dependency(Requires version 0.2.2 or higher):

    ```toml
    [dependencies]
    monoio = { version = "0.3.0", features = ["poll-io"] }
    ```

2. Import and use `IntoPollIo` to convert `CompIo` to `PollIo`.

    In the example below, we will start a listener, convert accepted connections to `PollIo`, and then use tokio’s `AsyncRead`/`AsyncWrite` traits for reading and writing.

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

    More examples can be found in [examples/h2_client.rs](../../examples/h2_client.rs) and [examples/h2_server.rs](../../examples/h2_server.rs).

## Underlying Implementation

Poll IO capability is implemented based on epoll over io-uring:

1. If the current driver is a legacy driver (e.g., on macOS or lower version Linux platforms), Poll IO capability can be provided directly;
2. If the current driver is an io-uring driver, enabling this feature will add an epoll fd to io-uring, and the Poll IO capability will be provided via this epoll fd.

When we convert a `CompIo` to `PollIo`, if running on io-uring, it is necessary to obtain the epoll fd maintained within and register the file descriptor corresponding to the `CompIo` with it. This way, when CompIo becomes ready, the runtime can detect the readiness of the epoll fd via io-uring’s completion notifications and call `epoll_wait(0)` to get all the ready events on it.

If the current driver is not io-uring, the conversion has almost no cost.

On Linux platforms, the relationship between IO traits and underlying drivers is as follows:

1. `CompIo` -> `io-uring` / `epoll`
2. `PollIo` -> `epoll` / `epoll over io-uring`

## Notes

1. This feature provides good support for libraries such as hyper and h2. However, to fully utilize io-uring’s capabilities, it is recommended to use CompIo-style IO interfaces (`AsyncReadRent`, `AsyncWriteRent`) in hot paths;
2. This feature has undergone extensive production environment validation, but some components may not yet support the `IntoPollIo`/`IntoCompIo` conversion capabilities. Please feel free to report any related issues.
