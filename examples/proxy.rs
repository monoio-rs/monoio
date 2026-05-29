//! An example TCP proxy.

use monoio::{
    io::{AsyncReadRent, AsyncWriteRent, AsyncWriteRentExt, Splitable},
    net::{TcpListener, TcpStream},
};

const LISTEN_ADDRESS: &str = "127.0.0.1:50005";
const TARGET_ADDRESS: &str = "127.0.0.1:50006";

#[monoio::main(entries = 512, timer_enabled = false)]
async fn main() {
    let listener = TcpListener::async_bind(LISTEN_ADDRESS)
        .await
        .unwrap_or_else(|_| panic!("[Server] Unable to bind to {LISTEN_ADDRESS}"));
    loop {
        if let Ok((in_conn, _addr)) = listener.accept().await {
            let out_conn = TcpStream::connect(TARGET_ADDRESS).await;
            if let Ok(out_conn) = out_conn {
                monoio::spawn(async move {
                    let (mut in_r, mut in_w) = in_conn.into_split();
                    let (mut out_r, mut out_w) = out_conn.into_split();
                    let _ = monoio::join!(
                        copy_one_direction(&mut in_r, &mut out_w),
                        copy_one_direction(&mut out_r, &mut in_w),
                    );
                    println!("relay finished");
                });
            } else {
                eprintln!("dial outbound connection failed");
            }
        } else {
            eprintln!("accept connection failed");
            return;
        }
    }
}

async fn copy_one_direction<FROM: AsyncReadRent, TO: AsyncWriteRent>(
    mut from: FROM,
    to: &mut TO,
) -> Result<Vec<u8>, std::io::Error> {
    let mut buf = Vec::with_capacity(8 * 1024);
    let mut res;
    loop {
        // read
        (res, buf) = from.read(buf).await;
        if res? == 0 {
            return Ok(buf);
        }

        // write all
        (res, buf) = to.write_all(buf).await;
        res?;

        // clear
        buf.clear();
    }
}
