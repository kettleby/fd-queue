// Copyright 2020 Steven Bosnick
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE-2.0 or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms

//! An implementation of `EnqueueFd` and `DequeueFd` that is integrated with tokio.

use std:: {
    convert::{TryFrom, TryInto},
    io::{Read, Write},
    os::unix::{
        io::{AsRawFd, RawFd},
        net::{SocketAddr, UnixStream as StdUnixStream},
    },
    net::Shutdown,
    path::Path,
    pin::Pin,
    task::{Context, Poll},
};


use futures_core::stream::Stream;
use futures_util::{future::poll_fn, ready};
use pin_project::pin_project;
use socket2::{Domain, SockAddr, Socket, Type};
use tokio::io::{self, AsyncRead, AsyncWrite, ReadBuf, unix::AsyncFd};

use crate::{DequeueFd, EnqueueFd, QueueFullError};

/// A structure representing a connected Unix socket.
///
/// This socket can be connected directly with UnixStream::connect or accepted from a listener with
/// UnixListener::incoming. Additionally, a pair of anonymous Unix sockets can be created with
/// UnixStream::pair.
#[pin_project]
#[derive(Debug)]
pub struct UnixStream {
    inner: AsyncFd<crate::net::UnixStream>,
}

/// A Unix socket which can accept connections from other Unix sockets.
///
/// You can accept a new connection by using the accept method. Alternatively UnixListener
/// implements the Stream trait, which allows you to use the listener in places that want a stream.
/// The stream will never return None and will also not yield the peer's SocketAddr structure.
/// Iterating over it is equivalent to calling accept in a loop.
#[derive(Debug)]
pub struct UnixListener {
    inner: AsyncFd<crate::net::UnixListener>,
}

// === impl UnixStream ===

impl UnixStream {
    /// Connects to the socket named by path.
    ///
    /// This function will create a new socket and connect the the path specifed,
    /// associating the returned stream with the default event loop's handle.
    pub async fn connect(path: impl AsRef<Path>) -> io::Result<UnixStream> {
        let typ = Type::stream().non_blocking().cloexec();
        let socket = Socket::new(Domain::unix(), typ, None)?;

        let addr = SockAddr::unix(path)?;
        match socket.connect(&addr) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
            Err(e) => return Err(e),
        }

        let stream: UnixStream = socket.into_unix_stream().try_into()?;
        poll_fn(|cx| stream.inner.poll_write_ready(cx)).await?.retain_ready();
        Ok(stream)
    }

    /// Creates an unnamed pair of conntected sockets.
    ///
    /// This function will create an unnamed pair of interconnected Unix sockets for
    /// communicating back and forth between one another. Each socket will be
    /// associted with the default event loop's handle.
    pub fn pair() -> io::Result<(UnixStream, UnixStream)> {
        let (stream1, stream2) = crate::net::UnixStream::pair()?;
        Ok((stream1.try_into()?, stream2.try_into()?))
    }

    /// Returns the socket address of the local half of this connection.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.inner.get_ref().local_addr()
    }

    /// Returns the socket address of the remote half of this conneciton.
    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        self.inner.get_ref().peer_addr()
    }

    /// Returns the value of the SO_ERROR option.
    pub fn take_error(&self) -> io::Result<Option<io::Error>> {
        self.inner.get_ref().take_error()
    }

    /// Shuts down the read, write, or both halves of this connection.
    ///
    /// This function will cause all pending and future I/O calls on the specified
    /// portions to immediately return with an appropriate value (see the
    /// documentation of `Shutdown`).
    pub fn shutdown(&self, how: Shutdown) -> io::Result<()> {
        self.inner.get_ref().shutdown(how)
    }
}

impl EnqueueFd for UnixStream {
    fn enqueue(&mut self, fd: &impl AsRawFd) -> Result<(), QueueFullError> {
        self.inner.get_mut().enqueue(fd)
    }
}

impl DequeueFd for UnixStream {
    fn dequeue(&mut self) -> Option<RawFd> {
        self.inner.get_mut().dequeue()
    }
}

impl AsRawFd for UnixStream {
    fn as_raw_fd(&self) -> RawFd {
        self.inner.get_ref().as_raw_fd()
    }
}

impl AsyncRead for UnixStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context,
        buf: &mut ReadBuf,
    ) -> Poll<io::Result<()>> {
        let inner = self.project().inner;

        let mut guard = ready!(inner.poll_read_ready_mut(cx))?;
        // TODO: add support for reading into uninitilized memory.
        let bufinit = buf.initialize_unfilled();
        match guard.try_io(|inner| inner.get_mut().read(bufinit)) {
            Err(_) => Poll::Pending,
            Ok(Err(e)) => Poll::Ready(Err(e)),
            Ok(Ok(count)) => {
                buf.advance(count);
                Poll::Ready(Ok(()))
            }
        }
    }
}

impl AsyncWrite for UnixStream {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context, buf: &[u8]) -> Poll<io::Result<usize>> {
        let inner = self.project().inner;

        let mut guard = ready!(inner.poll_write_ready_mut(cx))?;
        match guard.try_io(|inner| inner.get_mut().write(buf)) {
            Err(_) => Poll::Pending,
            Ok(Err(e)) => Poll::Ready(Err(e)),
            Ok(Ok(count)) => Poll::Ready(Ok(count)),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context) -> Poll<io::Result<()>> {
        let inner = self.project().inner;

        match inner.get_mut().shutdown(Shutdown::Write) {
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => Poll::Pending,
            Err(e) => Poll::Ready(Err(e)),
            Ok(()) => Poll::Ready(Ok(())),
        }
    }
}

impl TryFrom<crate::net::UnixStream> for UnixStream {
    type Error = io::Error;

    fn try_from(inner: crate::net::UnixStream) -> io::Result<UnixStream> {
        inner.set_nonblocking(true)?;
        Ok(UnixStream {
            inner: AsyncFd::new(inner)?,
        })
    }
}

impl TryFrom<StdUnixStream> for UnixStream {
    type Error = io::Error;

    fn try_from(inner: StdUnixStream) -> Result<Self, Self::Error> {
        let net_stream = crate::net::UnixStream::from(inner);
        net_stream.try_into()
    }
}

// === impl UnixListener ===

impl UnixListener {
    /// Creates a new UnixListener bound to the specified path.
    ///
    /// This function will bind a UnixListener to the specified path and associate it
    /// with the default event loop's handler.
    ///
    /// # Panics
    ///
    /// This function panics if thread-local runtime is not set.
    ///
    /// The runtime is usually set implicitly when this function is called from a
    /// future driven by a tokio runtime, otherwise runtime can be set explicitly
    /// with `Handle::enter` function.
    pub fn bind(path: impl AsRef<Path>) -> io::Result<UnixListener> {
        crate::net::UnixListener::bind(path)?.try_into()
    }

    /// Returns the local socket address of this listener.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.inner.get_ref().local_addr()
    }

    /// Returns the value of the `SO_ERROR` option.
    pub fn take_error(&self) -> io::Result<Option<io::Error>> {
        self.inner.get_ref().take_error()
    }

    /// Accepts a new incoming connection on this listener.
    ///
    /// # Panics
    ///
    /// This function panics if thread-local runtime is not set.
    ///
    /// The runtime is usually set implicitly when this function is called from a
    /// future driven by a tokio runtime, otherwise runtime can be set explicitly
    /// with `Handle::enter` function.
    pub async fn accept(&mut self) -> io::Result<(UnixStream, SocketAddr)> {
        poll_fn(|cx| self.poll_accept(cx)).await
    }

    fn poll_accept(&self, cx: &mut Context) -> Poll<io::Result<(UnixStream, SocketAddr)>> {
        let mut guard = ready!(self.inner.poll_read_ready(cx))?;

        match guard.try_io(|inner| inner.get_ref().accept()) {
            Err(_) => Poll::Pending,
            Ok(Err(e)) => Poll::Ready(Err(e)),
            Ok(Ok((socket, addr))) => Poll::Ready(Ok((socket.try_into()?, addr))),
        }
    }
}

impl AsRawFd for UnixListener {
    fn as_raw_fd(&self) -> RawFd {
        self.inner.get_ref().as_raw_fd()
    }
}

/// Produces a continuous stream of accepted connections.
///
/// This is the equivalent of calling `accept()` in a loop. It will never be ready
/// with `None`.
///
/// # Panics
///
/// Polling the stream panics if thread-local runtime is not set.
///
/// The runtime is usually set implicitly when this function is called from a
/// future driven by a tokio runtime, otherwise runtime can be set explicitly
/// with `Handle::enter` function.
impl Stream for UnixListener {
    type Item = io::Result<UnixStream>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Option<Self::Item>> {
        use Poll::{Pending, Ready};

        match self.poll_accept(cx) {
            Pending => Pending,
            Ready(Ok((stream, _))) => Ready(Some(Ok(stream))),
            Ready(Err(err)) => Ready(Some(Err(err))),
        }
    }
}

impl TryFrom<crate::net::UnixListener> for UnixListener {
    type Error = io::Error;

    fn try_from(inner: crate::net::UnixListener) -> io::Result<UnixListener> {
        inner.set_nonblocking(true)?;

        Ok(UnixListener {
            inner: AsyncFd::new(inner)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs::File;
    use std::io::{prelude::*, SeekFrom};
    use std::os::unix::io::FromRawFd as _;

    use tempfile::{tempdir, tempfile};
    use tokio::io::{AsyncWriteExt, AsyncReadExt};

    #[tokio::test]
    async fn unix_stream_reads_other_sides_writes() {
        let mut buf: [u8; 12] = [0; 12];

        let (mut sut, mut other) = UnixStream::pair().expect("Can't create UnixStream's");
        tokio::spawn(async move {
            other
                .write_all(b"Hello World!".as_ref())
                .await
                .expect("Can't write to UnixStream");
        });
        sut.read_exact(buf.as_mut())
            .await
            .expect("Can't read from UnixStream");

        assert_eq!(&buf, b"Hello World!");
    }

    #[tokio::test]
    async fn unix_stream_passes_fd() {
        let mut file1 = tempfile().expect("Can't create temp file.");
        file1
            .write_all(b"Hello World!\0")
            .expect("Can't write to temp file.");
        file1.flush().expect("Can't flush temp file.");
        file1
            .seek(SeekFrom::Start(0))
            .expect("Couldn't seek the file.");
        let mut buf = [0u8];

        let (mut sut, mut other) = UnixStream::pair().expect("Can't create UnixStream's");
        tokio::spawn(async move {
            other.enqueue(&file1).expect("Can't enqueue fd.");
            other
                .write_all(b"1".as_ref())
                .await
                .expect("Can't write to UnixStream");
        });
        sut.read_exact(buf.as_mut())
            .await
            .expect("Can't read from UnixStream");
        let fd = sut.dequeue().expect("Can't dequeue fd");

        let mut file2 = unsafe { File::from_raw_fd(fd) };
        let mut buf2 = Vec::new();
        file2.read_to_end(&mut buf2).expect("Can't read from file");
        assert_eq!(&buf2[..], b"Hello World!\0".as_ref());
    }

    #[tokio::test]
    async fn unix_stream_connects_to_listner() {
        let dir = tempdir().expect("Can't create temp dir");
        let sock_addr = dir.as_ref().join("socket");
        let mut buf: [u8; 12] = [0; 12];

        let mut listener = UnixListener::bind(&sock_addr).expect("Can't bind listener");
        tokio::spawn(async move {
            let mut client = UnixStream::connect(sock_addr)
                .await
                .expect("Can't connect to listener");
            client
                .write_all(b"Hello World!".as_ref())
                .await
                .expect("Can't write to client");
        });
        let (mut server, _) = listener.accept().await.expect("Can't accept on listener");
        server
            .read_exact(buf.as_mut())
            .await
            .expect("Can't read from server");

        assert_eq!(&buf, b"Hello World!");
    }
}
