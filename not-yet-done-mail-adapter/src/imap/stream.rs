//! The one stream type the session is generic over.
//!
//! `async_imap::Session<T>` needs a single concrete `T`, but an account may
//! speak implicit TLS, STARTTLS, or plain text on a loopback bridge. An enum
//! that forwards the two tokio traits keeps all three in one type without
//! boxing, and keeps `Debug` — which the session's bound insists on.

use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

use async_native_tls::TlsStream;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;

#[derive(Debug)]
pub(crate) enum MailStream {
    Tls(Box<TlsStream<TcpStream>>),
    Plain(TcpStream),
}

impl AsyncRead for MailStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.get_mut() {
            MailStream::Tls(s) => Pin::new(s.as_mut()).poll_read(cx, buf),
            MailStream::Plain(s) => Pin::new(s).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for MailStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            MailStream::Tls(s) => Pin::new(s.as_mut()).poll_write(cx, buf),
            MailStream::Plain(s) => Pin::new(s).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            MailStream::Tls(s) => Pin::new(s.as_mut()).poll_flush(cx),
            MailStream::Plain(s) => Pin::new(s).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            MailStream::Tls(s) => Pin::new(s.as_mut()).poll_shutdown(cx),
            MailStream::Plain(s) => Pin::new(s).poll_shutdown(cx),
        }
    }
}
