//! An example to illustrate selecting by macro or manually.

use std::{future::Future, pin::pin};

use monoio::{io::Canceller, net::TcpListener, time::Duration};
use pin_project_lite::pin_project;

#[monoio::main(enable_timer = true)]
async fn main() {
    // You can write poll by yourself
    let fut1 = monoio::time::sleep(Duration::from_secs(1));
    let fut2 = monoio::time::sleep(Duration::from_secs(20));
    let dual_select = DualSelect { fut1, fut2 };

    dual_select.await;
    println!("manually select returned");

    // Or you can use select macro
    let fut1 = monoio::time::sleep(Duration::from_secs(1));
    let fut2 = monoio::time::sleep(Duration::from_secs(20));
    monoio::select! {
        _ = fut1 => {
            println!("select returned with macro");
        }
        _ = fut2 => {
            println!("bug! something bad happened!");
        }
    }

    // A very important thing is if you use select to do timeout io,
    // you must cancel it manually.
    // In epoll, dropping a read future has no side effect since we
    // only wait for io ready, after waiting aborted, no syscall
    // will be made.
    // In io_uring, kernel may execute syscall at any time. So drop
    // a read future(even a AsyncCancel op will be issued after
    // dropping) does not mean it will be canceled instantly.
    // It may canceled, success or fail. We encourage you use our
    // cancelable io when you want to do io with timeout.
    //
    // Another option for timeout io is using readable/writable.
    // In this case you can sense io readiness. But it is not
    // a good way.
    let canceller = Canceller::new();
    let timeout_fut = monoio::time::sleep(Duration::from_millis(100));
    let listener = TcpListener::async_bind("127.0.0.1:0").await.unwrap();
    let mut io_fut = pin!(listener.cancelable_accept(canceller.handle()));

    monoio::select! {
        _ = timeout_fut => {
            println!("accept timeout but not sure!");
            canceller.cancel();

            // Here we must check io_fut again because it may succeed between timeout and canceled.
            // The io is canceled, so await for it is expected to return instantly.
            let res = io_fut.await;
            match res {
                Ok(_) => println!("connection accepted even timeout!"),
                Err(_) => println!("we are sure accept timeout!"),
            }
        }
        _ = &mut io_fut => {
            println!("connection accepted!");
        }
    }
}

pin_project! {
    struct DualSelect<F> {
        #[pin]
        fut1: F,
        #[pin]
        fut2: F
    }
}

impl<F> Future for DualSelect<F>
where
    F: Future<Output = ()>,
{
    type Output = ();

    fn poll(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        let this = self.project();

        match this.fut1.poll(cx) {
            std::task::Poll::Ready(_) => return std::task::Poll::Ready(()),
            std::task::Poll::Pending => {}
        }
        match this.fut2.poll(cx) {
            std::task::Poll::Ready(_) => std::task::Poll::Ready(()),
            std::task::Poll::Pending => std::task::Poll::Pending,
        }
    }
}
