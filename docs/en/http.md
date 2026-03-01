---
title: HTTP Server & HTTP Client
date: 2021-12-13 16:00:00
author: ihciah
---

This article briefly explains how to use HTTP in monoio. Related examples can be consulted:
1. [hyper_client.rs](/examples/hyper_client.rs)
2. [hyper_server.rs](/examples/hyper_server.rs)

## hyper

In Rust, http is often done using the `hyper` library. Maybe you will use libraries such as `reqwest` or `wrap` to implement HTTP requirements, but their underlying implementation is based on `hyper`.

Part of the code inside hyper is bound to tokio, but it also provides the freedom to replace Runtime to a certain extent. Tokio will not be introduced when the `runtime` feature is turned off.

In addition to the things that can be described by common traits, Runtime itself generally directly provides methods such as `spawn` and `sleep`. To decouple the runtime, the specific library must define its own traits and implement them for the known Runtime(or leave it to runtime developers). `hyper` uses the `Executor` trait to abstract the `spawn` method.

We can implement `Executor` like this:
```rust
#[derive(Clone)]
struct HyperExecutor;

impl<F> hyper::rt::Executor<F> for HyperExecutor
where
    F: Future +'static,
    F::Output:'static,
{
    fn execute(&self, fut: F) {
        monoio::spawn(fut);
    }
}
```

## server

The general logic of the server is to cyclically receive the connection, and then hand the connection to the corresponding service for processing; in order not to block subsequent connections, the processing will often be spawned out to complete.

We can use the `hyper::server::conn::Http::with_executor` method to specify the `Executor` used by hyper.

```rust
pub(crate) async fn serve_http<S, F, R, A>(addr: A, service: S) -> std::io::Result<()>
where
    S: FnMut(Request<Body>) -> F +'static + Copy,
    F: Future<Output = Result<Response<Body>, R>> +'static,
    R: std::error::Error +'static + Send + Sync,
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

Then provide the handler.

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

The client's implementation involves how to create a connection. This is also strongly coupled by Runtime, so hyper provides the `Connector` abstraction, which is defined as a Tower Service, which receives `hyper::Uri` and returns the implementation `tokio::io::AsyncRead + The future of tokio::io::AsyncWrite + hyper::client::connect::Connection` (that is, connection in a broad sense).

We implement monoio's IO object compatibility with hyper using `monoio_compat::hyper::MonoioIo`. For specific implementation refer to [`examples/hyper_client.rs`](/examples/hyper_client.rs).

Use `hyper::client::conn::http1::handshake` to create HTTP connection directly and make a request.

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

An additional note here is that hyper does not support thread-per-core Runtime well. You can refer to the [issue](https://github.com/hyperium/hyper/issues/2341) mentioned by the author of Glommio.

The Response of the Service that created the connection just mentioned is constrained to be `Send`; but obviously there is no way to do `Send` in monoio (or other Runtimes of the same model). But in fact it is indeed running in this thread, so in order to not change the hyper, we forcibly mark it as `Send`. This approach is not elegant at all and easily causes unsoundness, requiring users to strictly pay attention to their own code, and is not recommended for use in production. But it works.