// Copyright 2020 Steven Bosnick
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE-2.0 or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms

//! An implementation of `EnqueueFd` and `DequeueFd` that is integrated with tokio.

use std::convert::{TryFrom, TryInto};
use std::net::Shutdown;
use std::os::unix::{
    io::{AsRawFd, RawFd},
    net::SocketAddr,
};
use std::path::Path;
use std::pin::Pin;
use std::task::{Context, Poll};

use pin_project::pin_project;
use tokio::io::{self, AsyncRead, AsyncWrite, PollEvented};

use crate::{mio, DequeueFd, EnqueueFd, QueueFullError};

/// A structure representing a connected Unix socket.
///
/// This socket can be connected directly with UnixStream::connect or accepted from a listener with
/// UnixListener::incoming. Additionally, a pair of anonymous Unix sockets can be created with
/// UnixStream::pair.
#[pin_project]
#[derive(Debug)]
pub struct UnixStream {
    #[pin]
    inner: PollEvented<mio::UnixStream>,
}

/// A Unix socket which can accept connections from other Unix sockets.
///
/// You can accept a new connection by using the accept method. Alternatively UnixListener
/// implements the Stream trait, which allows you to use the listener in places that want a stream.
/// The stream will never return None and will also not yield the peer's SocketAddr structure.
/// Iterating over it is equivalent to calling accept in a loop.
pub struct UnixListener {}

// === impl UnixStream ===

impl UnixStream {
    /// Connects to the socket named by path.
    ///
    /// This function will create a new socket and connect the the path specifed,
    /// associating the returned stream with the default event loop's handle.
    ///
    /// For now, this is a synchronous function.
    pub fn connect(path: impl AsRef<Path>) -> io::Result<UnixStream> {
        mio::UnixStream::connect(path)?.try_into()
    }

    /// Creates an unnamed pair of conntected sockets.
    ///
    /// This function will create an unnamed pair of interconnected Unix sockets for
    /// communicating back and forth between one another. Each socket will be
    /// associted with the default event loop's handle.
    pub fn pair() -> io::Result<(UnixStream, UnixStream)> {
        let (stream1, stream2) = mio::UnixStream::pair()?;
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
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        self.project().inner.poll_read(cx, buf)
    }
}

impl AsyncWrite for UnixStream {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context, buf: &[u8]) -> Poll<io::Result<usize>> {
        self.project().inner.poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context) -> Poll<io::Result<()>> {
        self.project().inner.poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context) -> Poll<io::Result<()>> {
        self.project().inner.poll_shutdown(cx)
    }
}

impl TryFrom<mio::UnixStream> for UnixStream {
    type Error = io::Error;

    fn try_from(inner: mio::UnixStream) -> io::Result<UnixStream> {
        Ok(UnixStream {
            inner: PollEvented::new(inner)?,
        })
    }
}

// === impl UnixListener ===
// TODO: implment UnixListener

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs::File;
    use std::io::{prelude::*, SeekFrom};
    use std::os::unix::io::FromRawFd as _;

    use tokio::prelude::*;
    use tempfile::tempfile;

    #[tokio::test]
    async fn unix_stream_reads_other_sides_writes() {
        let mut buf: [u8;12] = [0;12];

        let (mut sut, mut other) = UnixStream::pair().expect("Can't create UnixStream's");
        tokio::spawn( async move {
            other.write_all(b"Hello World!".as_ref()).await.expect("Can't write to UnixStream");
        });
        sut.read_exact(buf.as_mut()).await.expect("Can't read from UnixStream");

        assert_eq!(&buf, b"Hello World!");
    }

    #[tokio::test]
    async fn unix_stream_passes_fd() {
        let mut file1 = tempfile().expect("Can't create temp file.");
        file1.write_all(b"Hello World!\0").expect("Can't write to temp file.");
        file1.flush().expect("Can't flush temp file.");
        file1.seek(SeekFrom::Start(0)).expect("Couldn't seek the file.");
        let mut buf = [0u8];

        let (mut sut, mut other) = UnixStream::pair().expect("Can't create UnixStream's");
        tokio::spawn( async move {
            other.enqueue(&file1).expect("Can't enqueue fd.");
            other.write_all(b"1".as_ref()).await.expect("Can't write to UnixStream");
        });
        sut.read_exact(buf.as_mut()).await.expect("Can't read from UnixStream");
        let fd = sut.dequeue().expect("Can't dequeue fd");

        let mut file2 = unsafe {File::from_raw_fd(fd)};
        let mut buf2 = Vec::new();
        file2.read_to_end(&mut buf2).expect("Can't read from file");
        assert_eq!(&buf2[..], b"Hello World!\0".as_ref());
    }
}
