//! Bound a stalled socket write even when the peer stops reading a response.
use std::{
    future::Future,
    io,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

pub(super) struct DeadlineIo<T> {
    inner: T,
    timeout: Duration,
    timer: Option<Pin<Box<tokio::time::Sleep>>>,
    expired: bool,
    metrics: Arc<super::metrics::Metrics>,
}
impl<T> DeadlineIo<T> {
    pub fn new(inner: T, timeout: Duration, metrics: Arc<super::metrics::Metrics>) -> Self {
        Self {
            inner,
            timeout,
            timer: None,
            expired: false,
            metrics,
        }
    }
    fn poll_result<R>(
        &mut self,
        cx: &mut Context<'_>,
        result: Poll<io::Result<R>>,
    ) -> Poll<io::Result<R>> {
        if result.is_ready() {
            self.timer = None;
            return result;
        }
        let timer = self
            .timer
            .get_or_insert_with(|| Box::pin(tokio::time::sleep(self.timeout)));
        if timer.as_mut().poll(cx).is_ready() {
            self.expired = true;
            super::metrics::increment(&self.metrics.write_timeouts);
            Poll::Ready(Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Socket write deadline exceeded",
            )))
        } else {
            Poll::Pending
        }
    }
    fn timeout_error<R>() -> Poll<io::Result<R>> {
        Poll::Ready(Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "Socket write deadline exceeded",
        )))
    }
}
impl<T: AsyncRead + Unpin> AsyncRead for DeadlineIo<T> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_read(cx, buf)
    }
}
impl<T: AsyncWrite + Unpin> AsyncWrite for DeadlineIo<T> {
    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }
    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        if this.expired {
            return Self::timeout_error();
        }
        let result = Pin::new(&mut this.inner).poll_write_vectored(cx, bufs);
        this.poll_result(cx, result)
    }
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        if this.expired {
            return Self::timeout_error();
        }
        let result = Pin::new(&mut this.inner).poll_write(cx, buf);
        this.poll_result(cx, result)
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if this.expired {
            return Self::timeout_error();
        }
        let result = Pin::new(&mut this.inner).poll_flush(cx);
        this.poll_result(cx, result)
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if this.expired {
            return Self::timeout_error();
        }
        let result = Pin::new(&mut this.inner).poll_shutdown(cx);
        this.poll_result(cx, result)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    #[tokio::test]
    async fn stalled_writes_expire_and_progress_resets_the_deadline() {
        let (stream, mut peer) = tokio::io::duplex(1);
        let metrics = Arc::new(super::super::metrics::Metrics::default());
        let mut stream = DeadlineIo::new(stream, Duration::from_millis(40), metrics.clone());
        stream.write_all(b"a").await.unwrap();
        let mut write = Box::pin(stream.write_all(b"b"));
        assert!(futures_util::poll!(&mut write).is_pending());
        let mut buf = [0];
        peer.read_exact(&mut buf).await.unwrap();
        write.await.unwrap();
        let error = tokio::time::timeout(Duration::from_secs(1), stream.write_all(b"c"))
            .await
            .unwrap()
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert_eq!(
            metrics
                .write_timeouts
                .load(std::sync::atomic::Ordering::Relaxed),
            1
        );
        assert!(stream.write_all(b"d").await.is_err());
        assert_eq!(
            metrics
                .write_timeouts
                .load(std::sync::atomic::Ordering::Relaxed),
            1
        );
    }

    #[tokio::test]
    async fn vectored_writes_preserve_transport_support_and_obey_deadlines() {
        let (stream, _peer) = tokio::io::duplex(1);
        let vectored = stream.is_write_vectored();
        let metrics = Arc::new(super::super::metrics::Metrics::default());
        let mut stream = DeadlineIo::new(stream, Duration::from_millis(40), metrics.clone());
        assert_eq!(stream.is_write_vectored(), vectored);
        let bufs = [io::IoSlice::new(b"a"), io::IoSlice::new(b"b")];
        let written =
            std::future::poll_fn(|cx| Pin::new(&mut stream).poll_write_vectored(cx, &bufs))
                .await
                .unwrap();
        assert_eq!(written, 1);
        let error = tokio::time::timeout(
            Duration::from_secs(1),
            std::future::poll_fn(|cx| Pin::new(&mut stream).poll_write_vectored(cx, &bufs)),
        )
        .await
        .unwrap()
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert_eq!(
            metrics
                .write_timeouts
                .load(std::sync::atomic::Ordering::Relaxed),
            1
        );
    }
}
