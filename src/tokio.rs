// Copyright 2020 Steven Bosnick
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE-2.0 or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms

//! An implementation of [`EnqueueFd`] and [`DequeueFd`] that is integrated with tokio.

use std::{
    convert::TryFrom,
    io::{ErrorKind, IoSlice, IoSliceMut},
    net::Shutdown,
    os::unix::{
        io::{AsRawFd, RawFd},
        net::{SocketAddr, UnixStream as StdUnixStream},
    },
    path::Path,
    pin::Pin,
    task::{Context, Poll},
};

use futures_core::stream::Stream;
use futures_util::ready;
use pin_project::pin_project;

use tokio::{
    io::{self, AsyncRead, AsyncWrite, Interest, ReadBuf},
    net::{
        unix::SocketAddr as TokioSocketAddr, UnixListener as TokioUnixListener,
        UnixStream as TokioUnixStream,
    },
};

use crate::{biqueue::BiQueue, DequeueFd, EnqueueFd, QueueFullError};

/// A structure representing a connected Unix socket with support for passing
/// [`RawFd`].
///
/// This is the implementation of [`EnqueueFd`] and [`DequeueFd`] that is based
/// on `tokio` [`UnixStream`][TokioUnixStream]. Conceptually the key interfaces
/// on `UnixStream` interact as shown in the following diagram:
///
/// ```text
/// EnqueueFd => AsyncWrite => AsyncRead => DequeueFd
/// ```
///
/// That is, you first enqueue a [`RawFd`] to the `UnixStream` and then
/// [`AsyncWrite`] at least one byte. On the other side  of the `UnixStream` you
/// then [`AsyncRead`] at least one byte and then dequeue the [`RawFd`].
///
/// This socket can be connected directly with [`UnixStream::connect`] or accepted
/// from a listener with [`UnixListener::accept`]. Additionally, a pair of
/// anonymous Unix sockets can be created with [`UnixStream::pair`].
///
/// # Examples
///
/// ```
/// # use fd_queue::{EnqueueFd, DequeueFd, tokio::UnixStream};
/// # use std::io::prelude::*;
/// # use std::os::unix::io::FromRawFd;
/// # use tempfile::tempfile;
/// use tokio::io::{AsyncReadExt, AsyncWriteExt};
/// use std::fs::File;
///
/// # tokio_test::block_on(async {
/// #
/// let (mut sock1, mut sock2) = UnixStream::pair()?;
///
/// // sender side
/// # let file1: File = tempfile()?;
/// // let file1: File = ...
/// sock1.enqueue(&file1).expect("Can't enqueue the file descriptor.");
/// sock1.write(b"a").await?;
/// sock1.flush().await?;
///
/// // receiver side
/// let mut buf = [0u8; 1];
/// sock2.read(&mut buf).await?;
/// let fd = sock2.dequeue().expect("Can't dequeue the file descriptor.");
/// let file2 = unsafe { File::from_raw_fd(fd) };
/// #
/// # Ok::<(), std::io::Error>(())
/// #
/// # });
/// # Ok::<(), std::io::Error>(())
/// ```
#[pin_project]
#[derive(Debug)]
pub struct UnixStream {
    #[pin]
    inner: TokioUnixStream,
    biqueue: BiQueue,
}

/// A Unix socket which can accept connections from other Unix sockets.
///
/// You can accept a new connection by using the accept method. Alternatively
/// UnixListener implements the Stream trait, which allows you to use the
/// listener in places that want a stream. The stream will never return None and
/// will also not yield the peer's SocketAddr structure. Iterating over it is
/// equivalent to calling accept in a loop.
///
/// # Examples
///
/// ```
/// # use tempfile::tempdir;
/// use fd_queue::tokio::{UnixStream, UnixListener};
/// use futures_util::stream::StreamExt;
/// use tokio::io::{AsyncReadExt, AsyncWriteExt};
///
/// # tokio_test::block_on(async {
/// # let dir = tempdir()?;
/// # let path = dir.path().join("mysock");
/// // let path: Path = ...
/// let mut listener = UnixListener::bind(&path)?;
///
/// tokio::spawn(async move {
///     let mut sock1 = UnixStream::connect(path).await?;
///     sock1.write(b"Hello World!").await?;
/// #   Ok::<(), std::io::Error>(())
/// });
///
/// let mut sock2 = listener.next().await.expect("Listener stream unexpectedly empty")?;
///
/// let mut buf = [0u8; 256];
/// sock2.read(&mut buf).await?;
///
/// assert!(buf.starts_with(b"Hello World!"));
/// #
/// # Ok::<(), std::io::Error>(())
/// # });
#[derive(Debug)]
pub struct UnixListener {
    inner: TokioUnixListener,
}

// === impl UnixStream ===

impl UnixStream {
    /// Connects to the socket named by path.
    ///
    /// This function will create a new socket and connect the the path specifed,
    /// associating the returned stream with the default event loop's handle.
    ///
    /// # Examples
    ///
    /// ```
    /// # use tempfile::tempdir;
    /// # use fd_queue::tokio::UnixListener;
    /// use fd_queue::tokio::UnixStream;
    /// # tokio_test::block_on(async {
    /// # let dir = tempdir()?;
    /// # let path = dir.path().join("mysock");
    /// // let path: Path = ...
    /// # let mut listener = UnixListener::bind(&path)?;
    /// # tokio::spawn(async move { listener.accept().await.expect("Can't accept")});
    ///
    /// UnixStream::connect(path).await?;
    /// #
    /// # Ok::<(), std::io::Error>(())
    /// # });
    /// ```
    pub async fn connect(path: impl AsRef<Path>) -> io::Result<UnixStream> {
        TokioUnixStream::connect(path).await.map(|s| s.into())
    }

    /// Creates an unnamed pair of conntected sockets.
    ///
    /// This function will create an unnamed pair of interconnected Unix sockets for
    /// communicating back and forth between one another. Each socket will be
    /// associted with the default event loop's handle.
    ///
    /// # Examples
    ///
    /// ```
    /// use fd_queue::tokio::UnixStream;
    ///
    /// # tokio_test::block_on(async {
    /// let (sock1, sock2) = UnixStream::pair()?;
    /// # Ok::<(), std::io::Error>(())
    /// # });
    /// ```
    pub fn pair() -> io::Result<(UnixStream, UnixStream)> {
        TokioUnixStream::pair().map(|(s1, s2)| (s1.into(), s2.into()))
    }

    /// Returns the socket address of the local half of this connection.
    ///
    /// # Examples
    ///
    /// ```
    /// # use tempfile::tempdir;
    /// # use fd_queue::tokio::UnixListener;
    /// use fd_queue::tokio::UnixStream;
    ///
    /// # tokio_test::block_on(async {
    /// # let dir = tempdir()?;
    /// # let path = dir.path().join("mysock");
    /// // let path: Path = ...
    /// # let mut listener = UnixListener::bind(&path)?;
    /// # tokio::spawn(async move { listener.accept().await.expect("Can't accept")});
    ///
    /// let sock = UnixStream::connect(path).await?;
    ///
    /// sock.local_addr()?;
    /// #
    /// # Ok::<(), std::io::Error>(())
    /// # });
    /// ```
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        to_addr(self.inner.local_addr()?)
    }

    /// Returns the socket address of the remote half of this conneciton.
    ///
    /// # Examples
    ///
    /// ```
    /// # use tempfile::tempdir;
    /// # use fd_queue::tokio::UnixListener;
    /// use fd_queue::tokio::UnixStream;
    ///
    /// # tokio_test::block_on(async {
    /// # let dir = tempdir()?;
    /// # let path = dir.path().join("mysock");
    /// // let path: Path = ...
    /// # let mut listener = UnixListener::bind(&path)?;
    /// # tokio::spawn(async move { listener.accept().await.expect("Can't accept")});
    ///
    /// let sock = UnixStream::connect(path).await?;
    ///
    /// sock.peer_addr()?;
    /// #
    /// # Ok::<(), std::io::Error>(())
    /// # });
    /// ```
    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        to_addr(self.inner.peer_addr()?)
    }

    /// Returns the value of the SO_ERROR option.
    ///
    /// # Examples
    ///
    /// ```
    /// # use tempfile::tempdir;
    /// # use fd_queue::tokio::UnixListener;
    /// use fd_queue::tokio::UnixStream;
    ///
    /// # tokio_test::block_on(async {
    /// # let dir = tempdir()?;
    /// # let path = dir.path().join("mysock");
    /// // let path: Path = ...
    /// # let mut listener = UnixListener::bind(&path)?;
    /// # tokio::spawn(async move { listener.accept().await.expect("Can't accept")});
    ///
    /// let sock = UnixStream::connect(path).await?;
    ///
    /// let err = match sock.take_error() {
    ///     Ok(Some(err)) => err,
    ///     Ok(None) => {
    ///         println!("No error found.");
    ///         return Ok(());
    ///     }
    ///     Err(e) => {
    ///         println!("Couldn't take the SO_ERROR option: {}", e);
    ///         return Ok(());
    ///     }
    /// };
    /// #
    /// # Ok::<(), std::io::Error>(())
    /// # });
    /// ```
    pub fn take_error(&self) -> io::Result<Option<io::Error>> {
        self.inner.take_error()
    }

    /// Shuts down the read, write, or both halves of this connection.
    ///
    /// This function will cause all pending and future I/O calls on the specified
    /// portions to immediately return with an appropriate value (see the
    /// documentation of `Shutdown`).
    ///
    /// # Examples
    ///
    /// ```
    /// use std::net::Shutdown;
    /// use tokio::io::AsyncReadExt;
    /// use fd_queue::tokio::UnixStream;
    ///
    /// # tokio_test::block_on(async {
    /// let (mut sock, _) = UnixStream::pair()?;
    ///
    /// sock.shutdown(Shutdown::Read)?;
    ///
    /// let mut buf = [0u8; 256];
    /// match sock.read(&mut buf).await {
    ///     Ok(0) => {},
    ///     _ => panic!("Read unexpectedly not shut down."),
    /// }
    /// #
    /// # Ok::<(), std::io::Error>(())
    /// # });
    /// ```
    pub fn shutdown(&self, how: Shutdown) -> io::Result<()> {
        shutdown(self, how)
    }
}

impl EnqueueFd for UnixStream {
    fn enqueue(&mut self, fd: &impl AsRawFd) -> Result<(), QueueFullError> {
        self.biqueue.enqueue(fd)
    }
}

impl DequeueFd for UnixStream {
    fn dequeue(&mut self) -> Option<RawFd> {
        self.biqueue.dequeue()
    }
}

impl AsRawFd for UnixStream {
    fn as_raw_fd(&self) -> RawFd {
        self.inner.as_raw_fd()
    }
}

impl AsyncRead for UnixStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context,
        buf: &mut ReadBuf,
    ) -> Poll<io::Result<()>> {
        let this = self.project();
        let inner = this.inner;
        let biqueue = this.biqueue;
        let fd = inner.as_raw_fd();

        loop {
            ready!(inner.poll_read_ready(cx))?;

            match inner.try_io(Interest::READABLE, || {
                // TODO: find a way to handle uninitialized memory on buf
                biqueue.read_vectored(fd, &mut [IoSliceMut::new(buf.initialize_unfilled())])
            }) {
                Ok(count) => {
                    buf.advance(count);
                    return Poll::Ready(Ok(()));
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => {}
                Err(e) => return Poll::Ready(Err(e)),
            }
        }
    }
}

impl AsyncWrite for UnixStream {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context, buf: &[u8]) -> Poll<io::Result<usize>> {
        self.poll_write_vectored(cx, &[IoSlice::new(buf)])
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context) -> Poll<io::Result<()>> {
        self.project().inner.poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context) -> Poll<io::Result<()>> {
        self.project().inner.poll_shutdown(cx)
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[std::io::IoSlice<'_>],
    ) -> Poll<Result<usize, std::io::Error>> {
        let this = self.project();
        let inner = this.inner;
        let biqueue = this.biqueue;
        let fd = inner.as_raw_fd();

        loop {
            ready!(inner.poll_write_ready(cx))?;

            match inner.try_io(Interest::WRITABLE, || biqueue.write_vectored(fd, bufs)) {
                Ok(count) => return Poll::Ready(Ok(count)),
                Err(e) if e.kind() == ErrorKind::WouldBlock => {}
                Err(e) => return Poll::Ready(Err(e)),
            }
        }
    }

    fn is_write_vectored(&self) -> bool {
        true
    }
}

impl From<TokioUnixStream> for UnixStream {
    fn from(inner: TokioUnixStream) -> UnixStream {
        UnixStream {
            inner,
            biqueue: BiQueue::new(),
        }
    }
}

impl TryFrom<StdUnixStream> for UnixStream {
    type Error = io::Error;

    fn try_from(inner: StdUnixStream) -> Result<Self, Self::Error> {
        inner.set_nonblocking(true)?;
        TokioUnixStream::from_std(inner).map(|stream| stream.into())
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
    ///
    /// # Examples
    ///
    /// ```
    /// # use tempfile::tempdir;
    /// use fd_queue::tokio::UnixListener;
    ///
    /// # tokio_test::block_on(async {
    /// # let dir = tempdir()?;
    /// # let path = dir.path().join("mysock");
    /// // let path: Path = ...
    /// let listener = UnixListener::bind(&path)?;
    /// #
    /// # Ok::<(), std::io::Error>(())
    /// # });
    pub fn bind(path: impl AsRef<Path>) -> io::Result<UnixListener> {
        TokioUnixListener::bind(path).map(|l| l.into())
    }

    /// Returns the local socket address of this listener.
    ///
    /// # Examples
    ///
    /// ```
    /// # use tempfile::tempdir;
    /// use fd_queue::tokio::UnixListener;
    ///
    /// # tokio_test::block_on(async {
    /// # let dir = tempdir()?;
    /// # let path = dir.path().join("mysock");
    /// // let path: Path = ...
    /// let listener = UnixListener::bind(&path)?;
    ///
    /// let addr = listener.local_addr()?;
    ///
    /// match addr.as_pathname() {
    ///     Some(path) => println!("The local address is {}.", path.display()),
    ///     None => println!("The local address does not have a pathname"),
    /// }
    /// #
    /// # Ok::<(), std::io::Error>(())
    /// # });
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        to_addr(self.inner.local_addr()?)
    }

    /// Returns the value of the `SO_ERROR` option.
    ///
    /// # Examples
    ///
    /// ```
    /// # use tempfile::tempdir;
    /// use fd_queue::tokio::UnixListener;
    ///
    /// # tokio_test::block_on(async {
    /// # let dir = tempdir()?;
    /// # let path = dir.path().join("mysock");
    /// // let path: Path = ...
    /// let listener = UnixListener::bind(&path)?;
    ///
    /// let so_error = listener.take_error()?;
    ///
    /// match so_error {
    ///     Some(err) => println!("The SO_ERROR was {}.", err),
    ///     None => println!("There was no SO_ERROR."),
    /// }
    /// #
    /// # Ok::<(), std::io::Error>(())
    /// # });
    pub fn take_error(&self) -> io::Result<Option<io::Error>> {
        self.inner.take_error()
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
    ///
    /// # Examples
    ///
    /// ```
    /// # use tempfile::tempdir;
    /// # use fd_queue::tokio::UnixStream;
    /// use fd_queue::tokio::UnixListener;
    ///
    /// # tokio_test::block_on(async {
    /// # let dir = tempdir()?;
    /// # let path = dir.path().join("mysock");
    /// // let path: Path = ...
    /// let mut listener = UnixListener::bind(&path)?;
    /// # tokio::spawn(async move { UnixStream::connect(path).await });
    ///
    /// let (sock, addr) = listener.accept().await?;
    /// #
    /// # Ok::<(), std::io::Error>(())
    /// # });
    pub async fn accept(&mut self) -> io::Result<(UnixStream, SocketAddr)> {
        self.inner
            .accept()
            .await
            .and_then(|(stream, addr)| to_addr(addr).map(|addr| (stream.into(), addr)))
    }

    fn poll_accept(&self, cx: &mut Context) -> Poll<io::Result<(UnixStream, SocketAddr)>> {
        self.inner.poll_accept(cx).map(|result| {
            result.and_then(|(stream, addr)| to_addr(addr).map(|addr| (stream.into(), addr)))
        })
    }
}

impl AsRawFd for UnixListener {
    fn as_raw_fd(&self) -> RawFd {
        self.inner.as_raw_fd()
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
///
/// # Examples
///
/// ```
/// # use tempfile::tempdir;
/// # use fd_queue::tokio::UnixStream;
/// use fd_queue::tokio::UnixListener;
/// use futures_util::stream::StreamExt;
///
/// # tokio_test::block_on(async {
/// # let dir = tempdir()?;
/// # let path = dir.path().join("mysock");
/// // let path: Path = ...
/// let mut listener = UnixListener::bind(&path)?;
/// # tokio::spawn(async move { UnixStream::connect(path).await });
///
/// let sock = listener.next().await.expect("Listener stream unexpectedly empty");
/// #
/// # Ok::<(), std::io::Error>(())
/// # });
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

impl From<TokioUnixListener> for UnixListener {
    fn from(inner: TokioUnixListener) -> UnixListener {
        UnixListener { inner }
    }
}

// === utility functions ===

fn to_addr(addr: TokioSocketAddr) -> io::Result<SocketAddr> {
    addr.as_pathname()
        .map_or(SocketAddr::from_pathname(""), |path| {
            SocketAddr::from_pathname(path)
        })
}

fn shutdown(socket: &impl AsRawFd, how: Shutdown) -> io::Result<()> {
    let how = match how {
        Shutdown::Write => libc::SHUT_WR,
        Shutdown::Read => libc::SHUT_RD,
        Shutdown::Both => libc::SHUT_RDWR,
    };
    // Safety: complies with the FFI documentation and the system call
    // check the validity of the parameters.
    let code = unsafe { libc::shutdown(socket.as_raw_fd(), how) };
    if code == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs::File;
    use std::io::{prelude::*, SeekFrom};
    use std::os::unix::io::FromRawFd as _;

    use tempfile::{tempdir, tempfile};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

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
