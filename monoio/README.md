# Monoio
A thread-per-core Rust runtime with io_uring/epoll/kqueue.

[![Crates.io][crates-badge]][crates-url]
[![MIT/Apache-2 licensed][license-badge]][license-url]
[![Build Status][actions-badge]][actions-url]
[![Codecov][codecov-badge]][codecov-url]
[![FOSSA Status](https://app.fossa.com/api/projects/git%2Bgithub.com%2Fbytedance%2Fmonoio.svg?type=shield)](https://app.fossa.com/projects/git%2Bgithub.com%2Fbytedance%2Fmonoio?ref=badge_shield)
[中文说明][zh-readme-url]

[crates-badge]: https://img.shields.io/crates/v/monoio.svg
[crates-url]: https://crates.io/crates/monoio
[license-badge]: https://img.shields.io/crates/l/monoio.svg
[license-url]: LICENSE-MIT
[actions-badge]: https://github.com/bytedance/monoio/actions/workflows/ci.yml/badge.svg
[actions-url]: https://github.com/bytedance/monoio/actions
[codecov-badge]: https://codecov.io/gh/bytedance/monoio/branch/master/graph/badge.svg?token=3MSAMJ6X3E
[codecov-url]: https://codecov.io/gh/bytedance/monoio
[zh-readme-url]: README-zh.md

## Design Goal
Monoio is a pure io_uring/epoll/kqueue Rust async runtime. Part of the design has been borrowed from Tokio and Tokio-uring. However, unlike Tokio-uring, Monoio does not run on top of another runtime, rendering it more efficient.

Moreover, Monoio is designed with a thread-per-core model in mind. Users do not need to worry about tasks being `Send` or `Sync`, as thread local storage can be used safely. In other words, the data does not escape the thread on await points, unlike on work-stealing runtimes such as Tokio. This is because for some use cases, specifically those targeted by this runtime, it is not necessary to make task schedulable between threads. For example, if we were to write a load balancer like NGINX, we would write it in a thread-per-core way. The thread local data does not need to be shared between threads, so the `Sync` and `Send` do not need to be implemented in the first place.

As you may have guessed, this runtime is primarily targeted at servers, where operations are io-bound on network sockets, and therefore the use of native asynchronous I/O APIs maximizes the throughput of the server. In order for Monoio to be as efficient as possible, we've enabled some unstable Rust features, and we've designed a whole new IO abstraction, which unfortunately may cause some compatibility problems. [Our benchmarks](https://github.com/bytedance/monoio/blob/master/docs/en/benchmark.md) prove that, for our use-cases, Monoio has a better performance than other Rust runtimes.

## Quick Start
To use monoio, you need rust 1.75. If you already installed it, please make sure it is the latest version.

Also, if you want to use io_uring, you must make sure your kernel supports it([5.6+](docs/en/platform-support.md)). And, memlock is [configured as a proper number](docs/en/memlock.md). If your kernel version does not meet the requirements, you can try to use the legacy driver to start, currently supports Linux and macOS([ref here](/docs/en/use-legacy-driver.md)).

🚧Experimental windows support is on the way.

Here is a basic example of how to use Monoio.

```rust,no_run
/// A echo example.
///
/// Run the example and `nc 127.0.0.1 50002` in another shell.
/// All your input will be echoed out.
use monoio::io::{AsyncReadRent, AsyncWriteRentExt};
use monoio::net::{TcpListener, TcpStream};

#[monoio::main]
async fn main() {
    let listener = TcpListener::bind("127.0.0.1:50002").unwrap();
    println!("listening");
    loop {
        let incoming = listener.accept().await;
        match incoming {
            Ok((stream, addr)) => {
                println!("accepted a connection from {}", addr);
                monoio::spawn(echo(stream));
            }
            Err(e) => {
                println!("accepted connection failed: {}", e);
                return;
            }
        }
    }
}

async fn echo(mut stream: TcpStream) -> std::io::Result<()> {
    let mut buf: Vec<u8> = Vec::with_capacity(8 * 1024);
    let mut res;
    loop {
        // read
        (res, buf) = stream.read(buf).await;
        if res? == 0 {
            return Ok(());
        }

        // write all
        (res, buf) = stream.write_all(buf).await;
        res?;

        // clear
        buf.clear();
    }
}
```

You can find more example code in `examples` of this repository.

## Limitations
1. On Linux 5.6 or newer, Monoio can use uring or epoll as io driver. On lower versions of Linux, it can only run in epoll mode. On macOS, kqueue can be used. Other platforms are currently not supported.
2. Monoio can not solve all problems. If the workload is very unbalanced, it may cause performance degradation than Tokio since CPU cores may not be fully utilized.

## Contributors
<a href="https://github.com/bytedance/monoio/graphs/contributors"><img src="https://opencollective.com/monoio/contributors.svg?width=890&button=false" /></a>

Thanks for their contributions!

## Associated Projects
- [local-sync](https://github.com/monoio-rs/local-sync): A thread local channel.
- [monoio-tls](https://github.com/monoio-rs/monoio-tls): TLS wrapper for Monoio.
- [monoio-codec](https://github.com/monoio-rs/monoio-codec): Codec utility for Monoio.

HTTP framework and RPC framework are on the way.

## Licenses
Monoio is licensed under the MIT license or Apache license.

During developing we referenced a lot from Tokio, Mio, Tokio-uring and other related projects. We would like to thank the authors of these projects.


[![FOSSA Status](https://app.fossa.com/api/projects/git%2Bgithub.com%2Fbytedance%2Fmonoio.svg?type=large)](https://app.fossa.com/projects/git%2Bgithub.com%2Fbytedance%2Fmonoio?ref=badge_large)
