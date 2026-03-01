---
title: HTTP 服务端 & HTTP 客户端
date: 2021-12-13 16:00:00
author: ihciah
---

本文简要说明如何在 monoio 中使用 HTTP。相关 examples 可以查阅:
1. [hyper_client.rs](/examples/hyper_client.rs)
2. [hyper_server.rs](/examples/hyper_server.rs)

## hyper

在 Rust 中 http 往往使用 `hyper` 库完成。也许你会使用 `reqwest` 或 `wrap` 等库来实现 HTTP 需求，但他们底层都是基于 `hyper` 实现的。

hyper 内部部分代码绑定了 tokio，但一定程度上也提供了替换 Runtime 的自由度，关闭 `runtime` feature 的情况下是不会引入 tokio 的。

除了可以使用通用 trait 描述的东西，Runtime 本身一般会直接提供 `spawn`、`sleep` 等方法，要解耦 runtime 必须由具体库自己定义 trait 并为已知 Runtime 实现（或者交给 Runtime 作者实现）。在 `hyper` 内部通过 `Executor` trait 来抽象 `spawn` 方法。

我们可以这样实现 Executor：
```rust
#[derive(Clone)]
struct HyperExecutor;

impl<F> hyper::rt::Executor<F> for HyperExecutor
where
    F: Future + 'static,
    F::Output: 'static,
{
    fn execute(&self, fut: F) {
        monoio::spawn(fut);
    }
}
```

## server

服务端的一般逻辑是循环接收连接，然后把连接交给对应的服务处理；为了不阻塞后续连接，往往处理会 spawn 出去完成。

我们可以通过 `hyper::server::conn::Http::with_executor` 方法来指定 hyper 使用的 Executor。

```rust
pub(crate) async fn serve_http<S, F, R, A>(addr: A, service: S) -> std::io::Result<()>
where
    S: FnMut(Request<Body>) -> F + 'static + Copy,
    F: Future<Output = Result<Response<Body>, R>> + 'static,
    R: std::error::Error + 'static + Send + Sync,
    A: Into<SocketAddr>,
{
    let listener = TcpListener::bind(addr.into())?;
    loop {
        let (stream, _) = listener.accept().await?;
        monoio::spawn(
            Http::new()
                .with_executor(HyperExecutor)
                .serve_connection(TcpStreamCompat::from(stream), service_fn(service)),
        );
    }
}
```

之后提供 handler 即可。

```rust
async fn hyper_handler(req: Request<Body>) -> Result<Response<Body>, std::convert::Infallible> {
    match (req.method(), req.uri().path()) {
        (&Method::GET, "/") => Ok(Response::new(Body::from("Hello World!"))),
        (&Method::GET, "/monoio") => Ok(Response::new(Body::from("Hello Monoio!"))),
        _ => Ok(Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Body::from("404 not found"))
            .unwrap()),
    }
}
```

## client

客户端的实现涉及如何创建连接，这个也是 Runtime 强耦合的，所以 hyper 提供了 `Connector` 抽象，它被定义为一个 Tower Service，接收 `hyper::Uri` 返回实现 `tokio::io::AsyncRead + tokio::io::AsyncWrite + hyper::client::connect::Connection`（也就是广义上的连接）的 future。

我们基于 `monoio_compat::hyper::MonoioIo` 来实现 monoio 的 IO 对象与 hyper 的兼容。具体实现参考 [`examples/hyper_client.rs`](/examples/hyper_client.rs)）。

使用 `hyper::client::conn::http1::handshake` 直接创建 HTTP 连接并发起请求。

```rust
let stream = TcpStream::connect(addr).await?.into_poll_io()?;
let io = MonoioIo::new(stream);

let (mut sender, conn) = hyper::client::conn::http1::handshake(io).await?;
monoio::spawn(async move {
    if let Err(err) = conn.await {
        println!("Connection failed: {:?}", err);
    }
});

let req = Request::builder()
    .uri(path)
    .header(hyper::header::HOST, authority.as_str())
    .body(Empty::<Bytes>::new())?;

let mut res = sender.send_request(req).await?;
```

这里要额外说明的是，hyper 并没有很好地支持 thread-per-core 的 Runtime。可以参考 Glommio 的作者提的 [issue](https://github.com/hyperium/hyper/issues/2341)。

刚刚提到的创建连接的 Service 它的 Response 被约束为是 `Send` 的；但明显在 monoio（或其他同样模型的 Runtime）中没办法做到 `Send`。但是事实上它又是确实在本线程运行的，所以为了在不改动 hyper 的情况下，我们强行标记其是 `Send` 的。这种做法一点也不优雅，容易造成 unsoundness，需要使用者严格注意自己的代码，不推荐在生产中使用。