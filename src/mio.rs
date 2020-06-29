// Copyright 2020 Steven Bosnick
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE-2.0 or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms

//! An implementation of `EnqueueFd` and `DequeueFd` that is integrated with mio.

use crate::{DequeueFd, EnqueueFd, QueueFullError};

use std::convert::{TryFrom, TryInto};
use std::io::{self, prelude::*, IoSlice, IoSliceMut};
use std::net::Shutdown;
use std::os::unix::io::{AsRawFd, FromRawFd, IntoRawFd, RawFd};
use std::os::unix::net::{SocketAddr, UnixListener as StdUnixListner, UnixStream as StdUnixStream};
use std::path::Path;

use mio::{event::Source, unix::SourceFd, Interest, Registry, Token};

/// A non-blocking Unix stream socket with support for passing [`RawFd`][RawFd].
///
/// [RawFd]: https://doc.rust-lang.org/stable/std/os/unix/io/type.RawFd.html
#[derive(Debug)]
pub struct UnixStream {
    inner: crate::UnixStream,
}

/// A non-blocking Unix domain socket server with support for passing [`RawFd`][RawFd].
///
/// [RawFd]: https://doc.rust-lang.org/stable/std/os/unix/io/type.RawFd.html
#[derive(Debug)]
pub struct UnixListener {
    inner: crate::UnixListener,
}

// === impl UnixStream ===
impl UnixStream {
    /// Connects to the socket named by `path`.
    ///
    /// Note that this is synchronous.
    pub fn connect(path: impl AsRef<Path>) -> io::Result<UnixStream> {
        StdUnixStream::connect(path)?.try_into()
    }

    /// Creates an unnamed pair of connected sockets.
    pub fn pair() -> io::Result<(UnixStream, UnixStream)> {
        let (sock1, sock2) = StdUnixStream::pair()?;

        Ok((sock1.try_into()?, sock2.try_into()?))
    }

    /// Returns the socket address of the local half of this connection.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.inner.local_addr()
    }

    /// Returns the socket address of the remote half of this connections.
    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        self.inner.peer_addr()
    }

    /// Returns the value of the `SO_ERROR` option.
    pub fn take_error(&self) -> io::Result<Option<io::Error>> {
        self.inner.take_error()
    }

    /// Shuts down the read, write, or both halves of the connection.
    ///
    /// This function will cause all pending and future I/O calls on the specified portions to
    /// immediately return with an appropriate value (see the documentation of `Shutdown`).
    pub fn shutdown(&self, how: Shutdown) -> io::Result<()> {
        self.inner.shutdown(how)
    }
}

impl EnqueueFd for UnixStream {
    fn enqueue(&mut self, fd: &impl AsRawFd) -> Result<(), QueueFullError> {
        self.inner.enqueue(fd)
    }
}

impl DequeueFd for UnixStream {
    fn dequeue(&mut self) -> Option<RawFd> {
        self.inner.dequeue()
    }
}

impl Read for UnixStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf)
    }

    fn read_vectored(&mut self, bufs: &mut [IoSliceMut]) -> io::Result<usize> {
        self.inner.read_vectored(bufs)
    }
}

impl Write for UnixStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.write(buf)
    }

    fn write_vectored(&mut self, bufs: &[IoSlice]) -> io::Result<usize> {
        self.inner.write_vectored(bufs)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

impl Source for UnixStream {
    fn register(
        &mut self,
        registry: &Registry,
        token: Token,
        interests: Interest,
    ) -> io::Result<()> {
        SourceFd(&self.as_raw_fd()).register(registry, token, interests)
    }

    fn reregister(
        &mut self,
        registry: &Registry,
        token: Token,
        interests: Interest,
    ) -> io::Result<()> {
        SourceFd(&self.as_raw_fd()).reregister(registry, token, interests)
    }

    fn deregister(&mut self, registry: &Registry) -> io::Result<()> {
        SourceFd(&self.as_raw_fd()).deregister(registry)
    }
}

impl AsRawFd for UnixStream {
    fn as_raw_fd(&self) -> RawFd {
        self.inner.as_raw_fd()
    }
}

/// Create a `UnixStream` from a `RawFd`.
///
/// This does not change the `RawFd` into non-blocking mode. It assumes that any such
/// required change has already been done.
impl FromRawFd for UnixStream {
    unsafe fn from_raw_fd(fd: RawFd) -> Self {
        let inner = StdUnixStream::from_raw_fd(fd);
        UnixStream {
            inner: inner.into(),
        }
    }
}

impl IntoRawFd for UnixStream {
    fn into_raw_fd(self) -> RawFd {
        self.inner.into_raw_fd()
    }
}

impl TryFrom<StdUnixStream> for UnixStream {
    type Error = io::Error;

    fn try_from(inner: StdUnixStream) -> io::Result<UnixStream> {
        inner.set_nonblocking(true)?;

        Ok(UnixStream {
            inner: inner.into(),
        })
    }
}

// === impl UnixListener ===

impl UnixListener {
    /// Creates a new `UnixListener` bound to the specific path.
    ///
    /// The listener will be set to non-blocking mode.
    pub fn bind(path: impl AsRef<Path>) -> io::Result<UnixListener> {
        StdUnixListner::bind(path)?.try_into()
    }

    /// Accepts a new incoming conneciton to this listener.
    ///
    /// The returned stream will be set to non-blocking mode.
    pub fn accept(&self) -> io::Result<(UnixStream, SocketAddr)> {
        self.inner.accept().and_then(|(stream, addr)| {
            stream.set_nonblocking(true)?;
            Ok((UnixStream { inner: stream }, addr))
        })
    }

    /// Returns the local socket address for this listener.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.inner.local_addr()
    }

    /// Returns the value of the `SO_ERROR` option.
    pub fn take_error(&self) -> io::Result<Option<io::Error>> {
        self.inner.take_error()
    }
}

impl AsRawFd for UnixListener {
    fn as_raw_fd(&self) -> RawFd {
        self.inner.as_raw_fd()
    }
}

/// Create a `UnixListener` from a `RawFd`.
///
/// This does not change the `RawFd` into non-blocking mode. It assumes that any such
/// required change has already been done.
impl FromRawFd for UnixListener {
    unsafe fn from_raw_fd(fd: RawFd) -> Self {
        let inner = StdUnixListner::from_raw_fd(fd);
        UnixListener {
            inner: inner.into(),
        }
    }
}

impl IntoRawFd for UnixListener {
    fn into_raw_fd(self) -> RawFd {
        self.inner.into_raw_fd()
    }
}

impl Source for UnixListener {
    fn register(
        &mut self,
        registry: &Registry,
        token: Token,
        interests: Interest,
    ) -> io::Result<()> {
        SourceFd(&self.as_raw_fd()).register(registry, token, interests)
    }

    fn reregister(
        &mut self,
        registry: &Registry,
        token: Token,
        interests: Interest,
    ) -> io::Result<()> {
        SourceFd(&self.as_raw_fd()).reregister(registry, token, interests)
    }

    fn deregister(&mut self, registry: &Registry) -> io::Result<()> {
        SourceFd(&self.as_raw_fd()).deregister(registry)
    }
}

impl TryFrom<StdUnixListner> for UnixListener {
    type Error = io::Error;

    fn try_from(inner: StdUnixListner) -> Result<Self, Self::Error> {
        inner.set_nonblocking(true)?;

        Ok(UnixListener {
            inner: inner.into(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::io::ErrorKind;
    use std::time::Duration;

    use assert_matches::assert_matches;
    use mio::{Events, Poll};

    #[test]
    fn stream_would_block_before_send() {
        let mut buf = [0; 1024];

        let (mut sut, _other) = UnixStream::pair().expect("Unable to create pair.");
        let result = sut.read(buf.as_mut());

        assert_matches!(result, Err(io) => assert_eq!(io.kind(), ErrorKind::WouldBlock));
    }

    #[test]
    fn stream_is_ready_for_read_after_write() {
        let mut poll = Poll::new().expect("Can't create poll.");
        let mut events = Events::with_capacity(5);

        let (mut sut, mut other) = UnixStream::pair().expect("Unable to create pair.");
        poll.registry()
            .register(&mut sut, Token(0), Interest::READABLE)
            .unwrap();
        write_to_steam(&mut other);

        let mut count = 0;
        loop {
            poll.poll(&mut events, Some(Duration::from_secs(1)))
                .unwrap();
            count += 1;
            if count > 500 {
                panic!("Too many spurious wakeups.");
            }

            for event in &events {
                if event.token() == Token(0) && event.is_readable() {
                    return;
                }
            }
        }
    }

    fn write_to_steam(stream: &mut UnixStream) {
        let mut count = 0;
        loop {
            count += 1;
            if count > 500 {
                panic!("Unable to write to steam after 500 tries");
            }

            match stream.write(b"hello".as_ref()) {
                Ok(_) => return,
                Err(ref e) if e.kind() == ErrorKind::WouldBlock => {}
                Err(_) => panic!("Unable to write to stream"),
            }
        }
    }
}
