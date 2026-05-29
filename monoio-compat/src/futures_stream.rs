use std::{
    pin::Pin,
    task::{Context, Poll},
};

use monoio::io::stream::Stream;

#[must_use = "streams do nothing unless polled"]
pub struct FuturesCompat<St, Item> {
    stream: St,
    write_queue: VecDeque<Item>,
}

impl<St> FuturesCompat<St, Item>
where
    St: Stream,
{
    pub fn new(stream: St) -> Self {
        Self {
            stream,
            write_queue: VecDeque::new(),
        }
    }
}

impl<St, Item> futures::Stream for FuturesCompat<St, Item>
where
    St: Stream + Unpin,
    Item: Unpin,
{
    type Item = St::Item;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let future = self.get_mut().stream.next();
        pin_mut!(future);
        future.poll(cx)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.stream.size_hint()
    }
}

impl<St, Item> futures::Sink<Item> for FuturesCompat<St, Item>
where
    St: Sink<Item> + Unpin,
    Item: Unpin,
{
    type Error = St::Error;

    fn poll_ready(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn start_send(self: Pin<&mut Self>, item: Item) -> Result<(), Self::Error> {
        self.get_mut().write_queue.push_back(item);
        Ok(())
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        let this = self.get_mut();

        while let Some(item) = this.write_queue.pop_front() {
            let future = this.stream.send(item);
            pin_mut!(future);
            match future.poll(cx) {
                Poll::Ready(Ok(())) => continue,
                Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                Poll::Pending => return Poll::Pending,
            }
        }

        let future = this.stream.flush();
        pin_mut!(future);
        future.poll(cx)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        let future = self.get_mut().stream.close();
        pin_mut!(future);
        future.poll(cx)
    }
}
