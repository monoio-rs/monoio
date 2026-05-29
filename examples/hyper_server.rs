//! HTTP server example with hyper in poll-io mode.
//!
//! After running this example, you can open http://localhost:23300
//! and http://localhost:23300/monoio in your browser or curl it.

use std::net::SocketAddr;

use bytes::Bytes;
use futures::Future;
use hyper::{server::conn::http1, service::service_fn};
use monoio::{io::IntoPollIo, net::TcpListener};

pub(crate) async fn serve_http<S, F, E, A>(addr: A, service: S) -> std::io::Result<()>
where
    S: Copy + Fn(Request<hyper::body::Incoming>) -> F + 'static,
    F: Future<Output = Result<Response<Full<Bytes>>, E>> + 'static,
    E: std::error::Error + 'static + Send + Sync,
    A: Into<SocketAddr>,
{
    let listener = TcpListener::async_bind(addr.into()).await?;
    loop {
        let (stream, _) = listener.accept().await?;
        let stream_poll = monoio_compat::hyper::MonoioIo::new(stream.into_poll_io()?);
        monoio::spawn(async move {
            // Handle the connection from the client using HTTP1 and pass any
            // HTTP requests received on that connection to the `hello` function
            if let Err(err) = http1::Builder::new()
                .timer(monoio_compat::hyper::MonoioTimer)
                .serve_connection(stream_poll, service_fn(service))
                .await
            {
                println!("Error serving connection: {:?}", err);
            }
        });
    }
}

use http_body_util::Full;
use hyper::{Method, Request, Response, StatusCode};

async fn hyper_handler(
    req: Request<hyper::body::Incoming>,
) -> Result<Response<Full<Bytes>>, std::convert::Infallible> {
    match (req.method(), req.uri().path()) {
        (&Method::GET, "/") => Ok(Response::new(Full::new(Bytes::from("Hello World!")))),
        (&Method::GET, "/monoio") => Ok(Response::new(Full::new(Bytes::from("Hello Monoio!")))),
        _ => Ok(Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Full::new(Bytes::from("404 not found")))
            .unwrap()),
    }
}

#[monoio::main(threads = 2, timer_enabled = true)]
async fn main() {
    println!("Running http server on 0.0.0.0:23300");
    let _ = serve_http(([0, 0, 0, 0], 23300), hyper_handler).await;
    println!("Http server stopped");
}
