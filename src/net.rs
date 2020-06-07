// Copyright 2020 Steven Bosnick
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE-2.0 or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms

use std::collections::VecDeque;
use std::fmt;
use std::io::{self, Error, ErrorKind, IoSlice, IoSliceMut, prelude::*};
use std::mem::size_of;
use std::net::Shutdown;
use std::os::unix::io::{AsRawFd, FromRawFd, IntoRawFd, RawFd};
use std::os::unix::net::{SocketAddr, UnixListener as StdUnixListner, UnixStream as StdUnixStream};
use std::path::Path;
use std::slice;

use nix::cmsg_space;
use nix::sys::socket::{recvmsg, sendmsg, ControlMessage, ControlMessageOwned, MsgFlags};
use nix::sys::uio::IoVec;

use crate::{EnqueueFd, DequeueFd, QueueFullError};

/// A structure representing a connected Unix socket with support for passing
/// [`RawFd`][RawFd].
///
/// This is the primary implementation of `EnqueueFd` and `DequeueFd` and it is based
/// on a blocking, Unix domain socket. Conceptually the key interfaces on
/// `UnixStream` interact as shown in the following diagram:
///
/// ```text
/// EnqueueFd => Write => Read => DequeueFd
/// ```
///
/// That is, you first endqueue a [`RawFd`][RawFd] to the `UnixStream` and then
/// `Write` at least one byte. On the other side of the `UnixStream` you then `Read`
/// at least one byte and then dequeue the [`RawFd`][RawFd].
///
/// # Examples
///
/// ```
/// # use fd_queue::{EnqueueFd, DequeueFd, UnixStream};
/// # use std::io::prelude::*;
/// # use std::os::unix::io::FromRawFd;
/// # use tempfile::tempfile;
/// use std::fs::File;
///
/// let (mut sock1, mut sock2) = UnixStream::pair()?;
///
/// // sender side
/// # let file1: File = tempfile()?;
/// // let file1: File = ...
/// sock1.enqueue(&file1).expect("Can't endqueue the file descriptor.");
/// sock1.write(b"a")?;
/// sock1.flush()?;
///
/// // receiver side
/// let mut buf = [0u8; 1];
/// sock2.read(&mut buf)?;
/// let fd = sock2.dequeue().expect("Can't dequeue the file descriptor.");
/// let file2 = unsafe { File::from_raw_fd(fd) };
///
/// # Ok::<(),std::io::Error>(())
/// ```
///
/// [RawFd]: https://doc.rust-lang.org/stable/std/os/unix/io/type.RawFd.html
#[derive(Debug)]
pub struct UnixStream {
    inner: StdUnixStream,
    infd: VecDeque<RawFd>,
    outfd: Option<Vec<RawFd>>,
    cmsg_buffer: Vec<u8>,
}

/// A structure representing a Unix domain socket server whose connected sockets
/// have support for passing [`RawFd`][RawFd].
///
/// [RawFd]: https://doc.rust-lang.org/stable/std/os/unix/io/type.RawFd.html
#[derive(Debug)]
pub struct UnixListener {
    inner: StdUnixListner,
}

/// An iterator over incoming connections to a `UnixListener`.
///
/// It is an infinite iterator that will never return `None`
#[derive(Debug)]
pub struct Incoming<'a> {
    listener: &'a UnixListener,
}

#[derive(Debug)]
struct CMsgTruncatedError { }

// === impl UnixStream ===
impl UnixStream {

    /// The size of the bounded queue of outbound [`RawFd`][RawFd].
    ///
    /// [RawFd]: https://doc.rust-lang.org/stable/std/os/unix/io/type.RawFd.html
    pub const FD_QUEUE_SIZE: usize = 2;

    /// Connects to the socket named by `path`.
    ///
    /// # Examples
    ///
    /// ```
    /// # use std::thread;
    /// # use fd_queue::UnixListener;
    /// # use tempfile::tempdir;
    /// use fd_queue::UnixStream;
    ///
    /// # let dir = tempdir()?;
    /// # let path = dir.path().join("mysock");
    /// // let path = ...
    /// # let listener = UnixListener::bind(&path)?;
    /// # thread::spawn(move || listener.accept());
    ///
    /// let sock = match UnixStream::connect(path) {
    ///     Ok(sock) => sock,
    ///     Err(e) => {
    ///         println!("Couldn't connect to a socket: {}", e);
    ///         return Ok(());
    ///     }
    /// };
    ///
    /// # Ok::<(), std::io::Error>(())
    /// ```
    pub fn connect<P: AsRef<Path>>(path: P) -> io::Result<UnixStream> {
        StdUnixStream::connect(path).map(|s| s.into())
    }

    /// Creates an unnamed pair of connected sockets.
    ///
    /// Returns two `UnixStream`s which are connected to each other.
    ///
    /// # Examples
    ///
    /// ```
    /// use fd_queue::UnixStream;
    ///
    /// let (sock1, sock2) = match UnixStream::pair() {
    ///     Ok((sock1, sock2)) => (sock1, sock2),
    ///     Err(e) => {
    ///         println!("Couldn't create a pair of sockets: {}", e);
    ///         return;
    ///     }
    /// };
    /// ```
    pub fn pair() -> io::Result<(UnixStream, UnixStream)> {
        StdUnixStream::pair().map(|(s1, s2)| (s1.into(), s2.into()))
    }

    /// Creates a new independently owned handle to the underlying socket.
    ///
    /// The returned `UnixStream` is a reference to the same stream that this object references.
    /// Both handles will read and write the same stream of data, and options set on one stream
    /// will be propagated to the other stream.
    ///
    /// # Examples
    ///
    /// ```
    /// use fd_queue::UnixStream;
    ///
    /// let (sock1, _) = UnixStream::pair()?;
    ///
    /// let sock2 = match sock1.try_clone() {
    ///     Ok(sock) => sock,
    ///     Err(e) => {
    ///         println!("Couldn't clone a socket: {}", e);
    ///         return Ok(());
    ///     }
    /// };
    ///
    /// # Ok::<(),std::io::Error>(())
    /// ```
    pub fn try_clone(&self) -> io::Result<UnixStream> {
        self.inner.try_clone().map(|s| s.into())
    }

    /// Returns the socket address of the local half of this connection.
    ///
    /// # Examples
    ///
    /// ```
    /// # use std::thread;
    /// # use fd_queue::UnixListener;
    /// # use tempfile::tempdir;
    /// use fd_queue::UnixStream;
    ///
    /// # let dir = tempdir()?;
    /// # let path = dir.path().join("mysock");
    /// // let path = ...
    /// # let listener = UnixListener::bind(&path)?;
    /// # thread::spawn(move || listener.accept());
    /// #
    /// let sock = UnixStream::connect(path)?;
    ///
    /// let addr = match sock.local_addr() {
    ///     Ok(addr) => addr,
    ///     Err(e) => {
    ///         println!("Couldn't get the local address: {}", e);
    ///         return Ok(());
    ///     }
    /// };
    ///
    /// # Ok::<(),std::io::Error>(())
    /// ```
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.inner.local_addr()
    }

    /// Returns the socket address of the remote half of this connection.
    ///
    /// # Examples
    ///
    /// ```
    /// # use std::thread;
    /// # use fd_queue::UnixListener;
    /// # use tempfile::tempdir;
    /// use fd_queue::UnixStream;
    ///
    /// # let dir = tempdir()?;
    /// # let path = dir.path().join("mysock");
    /// // let path = ...
    /// # let listener = UnixListener::bind(&path)?;
    /// # thread::spawn(move || listener.accept());
    /// #
    /// let sock = UnixStream::connect(path)?;
    ///
    /// let addr = match sock.peer_addr() {
    ///     Ok(addr) => addr,
    ///     Err(e) => {
    ///         println!("Couldn't get the local address: {}", e);
    ///         return Ok(());
    ///     }
    /// };
    ///
    /// # Ok::<(),std::io::Error>(())
    /// ```
    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        self.inner.peer_addr()
    }

    /// Returns the value of the `SO_ERROR` option.
    ///
    /// # Examples
    ///
    /// ```
    /// use fd_queue::UnixStream;
    ///
    /// let (sock, _) = UnixStream::pair()?;
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
    ///
    /// # Ok::<(),std::io::Error>(())
    /// ```
    pub fn take_error(&self) -> io::Result<Option<Error>> {
        self.inner.take_error()
    }

    /// Shuts down the read, write, or both halves of this connection.
    ///
    /// This function will cause all pending and future I/O calls on the specified portions to
    /// immediately return with an appropriate value.
    ///
    /// # Examples
    ///
    /// ```
    /// use fd_queue::UnixStream;
    /// use std::net::Shutdown;
    /// use std::io::Read;
    ///
    /// let (mut sock, _) = UnixStream::pair()?;
    ///
    /// sock.shutdown(Shutdown::Read).expect("Couldn't shutdown.");
    ///
    /// let mut buf = [0u8; 256];
    /// match sock.read(buf.as_mut()) {
    ///     Ok(0) => {},
    ///     _ => panic!("Read unexpectly not shut down."),
    /// }
    ///
    /// # Ok::<(),std::io::Error>(())
    /// ```
    pub fn shutdown(&self, how: Shutdown) -> io::Result<()> {
        self.inner.shutdown(how)
    }
}

/// Enqueue a [`RawFd`][RawFd] for later transmission across the `UnixStream`.
///
/// The [`RawFd`][RawFd] will be transmitted on a later call to a method of `Write`.
/// The number of [`RawFd`][RawFd] that can be enqueued before being transmitted is
/// bounded by `FD_QUEUE_SIZE`.
///
/// [RawFd]: https://doc.rust-lang.org/stable/std/os/unix/io/type.RawFd.html
impl EnqueueFd for UnixStream {
    fn enqueue(&mut self, fd: &impl AsRawFd) -> std::result::Result<(), QueueFullError> {
        let outfd = self.outfd.get_or_insert_with(|| Vec::with_capacity(Self::FD_QUEUE_SIZE));
        if outfd.len() >= Self::FD_QUEUE_SIZE {
            Err(QueueFullError::new())
        } else {
            outfd.push(fd.as_raw_fd());
            Ok(())
        }
    }
}

/// Dequeue a [`RawFd`][RawFd] that was previously transmitted across the
/// `UnixStream`.
///
/// The [`RawFd`][RawFd] that are dequeued were transmitted by a previous call to a
/// method of `Read`.
///
/// [RawFd]: https://doc.rust-lang.org/stable/std/os/unix/io/type.RawFd.html
impl DequeueFd for UnixStream {
    fn dequeue(&mut self) -> Option<RawFd> {
        self.infd.pop_front()
    }
}

/// Receive bytes and [`RawFd`][RawFd] that are transmitted across the `UnixStream`.
///
/// The [`RawFd`][RawFd] that are received along with the bytes will be available
/// through the method of the `DequeueFd` implementation. The number of
/// [`RawFd`][RawFd] that can be received in a single call to one of the `Read`
/// methods is bounded by `FD_QUEUE_SIZE`. It is an error if the other side of this
/// `UnixStream` attempted to send more control messages (including [`RawFd`][RawFd])
/// than will fit in the buffer that has been sized for receiving up to
/// `FD_QUEUE_SIZE` [`RawFd`][RawFd].
///
/// [RawFd]: https://doc.rust-lang.org/stable/std/os/unix/io/type.RawFd.html
impl Read for UnixStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.read_vectored(&mut [IoSliceMut::new(buf)])
    }

    fn read_vectored(&mut self, bufs: &mut [IoSliceMut]) -> io::Result<usize> {
        assert_eq!(size_of::<IoSliceMut>(), size_of::<IoVec<&mut [u8]>>());
        assert!((isize::MAX as usize) / size_of::<IoVec<&mut [u8]>>() >= bufs.len());

        let bufs_ptr = bufs.as_mut_ptr();
        let vecs_ptr = bufs_ptr as *mut IoVec<&mut [u8]>;

        // Safety: from_raw_parts_mut(data, len) requires 3 things
        //
        //      1. data is valid (i.e. non-null and pointing to a single allocated
        //         object)
        //      2. the memory referenced by the returned slice must not be accessed
        //         other than through that slice for the lifetime of the slice
        //      3. len * size_of::<T>() <= isize::MAX
        //
        // For the first condition, vecs_ptr is is a pointer to the first byte of the
        // bufs slice which is bufs.len() * size_of::<IoSliceMut> bytes long. The
        // first assertion means that it is also bufs.len() *
        // size_of::<IoVec<&mut[u8]>> bytes long. vecs_ptr thus points to the single
        // allocated object bufs for its whole bufs.len * size_of::<IoVec<&mut[u8]>
        // size. Since it is pointing to the first byte of a slice, it is non-null.
        // It is properly aligned because it is equal to bufs_ptr (which is properly
        // aligned) and because both IoSliceMut and IoVec<&mut [u8]> are guarenteed
        // to have the same layout, specifically the layout of iovec C ABI type.
        //
        // For the second condition, all of bufs, bufs_ptr, vecs_ptr, and vecs
        // reference the same memory, but only vecs is used in any manner after this
        // call.
        //
        // For the third condition, the second assertion demonstarates that it holds
        // (absent a panic).
        let vecs = unsafe { slice::from_raw_parts_mut(vecs_ptr, bufs.len()) };

        let msg = recvmsg(self.as_raw_fd(), &vecs, Some(&mut self.cmsg_buffer), MsgFlags::empty())
            .map_err(map_error)?;
        if msg.flags.contains(MsgFlags::MSG_CTRUNC) {
            return Err(Error::new(ErrorKind::Other, CMsgTruncatedError::new()));
        }

        for c in msg.cmsgs() {
            if let ControlMessageOwned::ScmRights(fds) = c {
                self.infd.extend(fds);
            }
        }

        Ok(msg.bytes)
    }
}

/// Transmit bytes and [`RawFd`][RawFd] across the `UnixStream`.
///
/// The [`RawFd`][RawFd] that are transmitted along with the bytes are ones that were
/// previously enqueued for transmission through the method of `EnqueueFd`.
///
/// [RawFd]: https://doc.rust-lang.org/stable/std/os/unix/io/type.RawFd.html
impl Write for UnixStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.write_vectored(&[IoSlice::new(buf)])
    }

    fn write_vectored(&mut self, bufs: &[IoSlice]) -> io::Result<usize> {
        assert_eq!(size_of::<IoSlice>(), size_of::<IoVec<&[u8]>>());
        assert!((isize::MAX as usize) / size_of::<IoVec<&[u8]>>() >= bufs.len());

        let bufs_ptr = bufs.as_ptr();
        let vecs_ptr = bufs_ptr as *const IoVec<&[u8]>;

        // Safety: from_raw_parts(data, len) requires three things:
        //
        //      1. data must be valid for len * size_of::<T>() bytes (non-null,
        //         properly aligned, and the memory range from a single allocation).
        //      2. The memory reference by the return slice must not be mutated for
        //         the liftime of the slice.
        //      3. len * size_of::<T>() <= isize::MAX
        //
        // For the first condtion vecs_ptr is non-null because it points to the first
        // byte of bufs. It is properly aligned for IoVec<&[u8]> because it is
        // properly aligned for IoSlice and IoVec<&[u8]> and IoSlice both have the
        // same layout: the C ABI for an iovec. The first assertion above ensures
        // that bufs.len() * size_of::<IoVec<&[u8]>> is the same as bufs.len() *
        // size_of::<IoSlice> and both are the number of bytes in bufs, which is a
        // single allocation.
        //
        // For the second condition, bufs, bufs_ptr, vecs, and vecs_ptr all refer to
        // the same memory but they are all const pointers or shared references.
        // There are no other references to that memory in this function and any such
        // references in other functions must be through shared references (becuase
        // bufs is a shared reference).
        //
        // For the third condition, the second assertion ensurse this is true.
        let vecs = unsafe { slice::from_raw_parts(vecs_ptr, bufs.len()) };

        let outfd = self.outfd.take();

        let fds = outfd.unwrap_or_else(Vec::new);
        let cmsgs = if fds.is_empty() {
            Vec::new()
        } else {
            vec![ControlMessage::ScmRights(&fds)]
        };

        sendmsg(self.as_raw_fd(), &vecs, &cmsgs, MsgFlags::empty(), None)
            .map_err(map_error)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

impl AsRawFd for UnixStream {
    fn as_raw_fd(&self) -> RawFd {
        self.inner.as_raw_fd()
    }
}

impl FromRawFd for UnixStream {
    unsafe fn from_raw_fd(fd: RawFd) -> Self {
        StdUnixStream::from_raw_fd(fd).into()
    }
}

impl IntoRawFd for UnixStream {
    fn into_raw_fd(self) -> RawFd {
        self.inner.into_raw_fd()
    }
}

impl From<StdUnixStream> for UnixStream {
    fn from(inner: StdUnixStream) -> Self {
        Self{
            inner,
            infd: VecDeque::with_capacity(Self::FD_QUEUE_SIZE),
            outfd: None,
            cmsg_buffer: cmsg_space!([RawFd; Self::FD_QUEUE_SIZE]),
        }
    }
}

fn map_error(e: nix::Error) -> io::Error {
    use nix::Error::*;

    match e {
        Sys(e) => io::Error::from_raw_os_error(e as i32),
        _ => io::Error::new(io::ErrorKind::Other, Box::new(e)),
    }
}

// === impl UnixListener ===
impl UnixListener {

    /// Create a new `UnixListener` bound to the specified socket.
    ///
    /// # Examples
    ///
    /// ```
    /// use fd_queue::UnixListener;
    /// # use tempfile::tempdir;
    /// # let dir = tempdir()?;
    /// # let path = dir.path().join("mysocket");
    /// // let path = ...
    /// let listener = match UnixListener::bind(&path) {
    ///     Ok(listener) => listener,
    ///     Err(e) => {
    ///         println!("Can't bind the unix socket libtest: {}", e);
    ///         return Ok(());
    ///     }
    /// };
    ///
    /// # Ok::<(),std::io::Error>(())
    /// ```
    pub fn bind(path: impl AsRef<Path>) -> io::Result<UnixListener> {
        StdUnixListner::bind(path).map(|s| s.into())
    }

    /// Accepts a new incoming connection to this server.
    ///
    /// This function will block the calling thread until a new Unix connection is
    /// established. When established the corresponding `UnixStream` and the remote
    /// peer's address will be returned.
    ///
    /// # Examples
    ///
    /// ```
    /// use fd_queue::UnixListener;
    /// # use fd_queue::UnixStream;
    /// # use std::thread;
    /// # use tempfile::tempdir;
    /// # let dir = tempdir()?;
    /// # let path = dir.path().join("mysocket");
    ///
    /// // let path = ...
    /// let listener = UnixListener::bind(&path)?;
    /// # thread::spawn(move || UnixStream::connect(path).expect("Can't connect"));
    ///
    /// let (sock, addr) = match listener.accept() {
    ///     Ok((sock, addr)) => (sock, addr),
    ///     Err(e) => {
    ///         println!("Can't accept unix stream: {}", e);
    ///         return Ok(());
    ///     }
    /// };
    ///
    /// # Ok::<(),std::io::Error>(())
    /// ```
    pub fn accept(&self) -> io::Result<(UnixStream, SocketAddr)> {
        self.inner.accept().map(|(s, a)| (s.into(), a))
    }

    /// Create a new independently owned handle to the underlying socket.
    ///
    /// The returned `UnixListener` is a reference to the same socket that this
    /// object references. Both handles can be used to accept incoming connections
    /// and options set on one will affect the other.
    ///
    /// # Examples
    ///
    /// ```
    /// use fd_queue::UnixListener;
    /// # use tempfile::tempdir;
    /// # let dir = tempdir()?;
    /// # let path = dir.path().join("mysocket");
    ///
    /// // let path = ...
    /// let listener1 = UnixListener::bind(&path)?;
    ///
    /// let listener2 = match listener1.try_clone() {
    ///     Ok(listener) => listener,
    ///     Err(e) => {
    ///         println!("Can't clone listener: {}", e);
    ///         return Ok(());
    ///     }
    /// };
    ///
    /// # Ok::<(),std::io::Error>(())
    /// ```
    pub fn try_clone(&self) -> io::Result<UnixListener> {
        self.inner.try_clone().map(|s| s.into())
    }

    /// Returns the local address of of this listener.
    ///
    /// # Examples
    ///
    /// ```
    /// use fd_queue::UnixListener;
    /// # use tempfile::tempdir;
    /// # let dir = tempdir()?;
    /// # let path = dir.path().join("mysocket");
    ///
    /// // let path = ...
    /// let listener = UnixListener::bind(&path)?;
    ///
    /// let addr = match listener.local_addr() {
    ///     Ok(addr) => addr,
    ///     Err(e) => {
    ///         println!("Couldn't get local address: {}", e);
    ///         return Ok(());
    ///     }
    /// };
    ///
    /// # Ok::<(),std::io::Error>(())
    /// ```
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.inner.local_addr()
    }

    /// Return the value of the `SO_ERROR` option.
    ///
    /// # Examples
    ///
    /// ```
    /// use fd_queue::UnixListener;
    /// # use tempfile::tempdir;
    /// # let dir = tempdir()?;
    /// # let path = dir.path().join("mysocket");
    ///
    /// // let path = ...
    /// let listener = UnixListener::bind(&path)?;
    ///
    /// let err = match listener.take_error() {
    ///     Ok(Some(err)) => err,
    ///     Ok(None) => {
    ///         println!("There was no SO_ERROR option pending.");
    ///         return Ok(());
    ///     }
    ///     Err(e) => {
    ///         println!("Couldn't get the SO_ERROR option: {}", e);
    ///         return Ok(())
    ///     }
    /// };
    ///
    /// # Ok::<(),std::io::Error>(())
    /// ```
    pub fn take_error(&self) -> io::Result<Option<Error>> {
        self.inner.take_error()
    }

    /// Returns an iterator over incoming connections.
    ///
    /// The iterator will never return `None` and also will not yield the peer's
    /// [`SocketAddr`][SocketAddr] structure.
    ///
    /// # Examples
    ///
    /// ```
    /// use fd_queue::UnixListener;
    /// # use fd_queue::UnixStream;
    /// # use std::thread;
    /// # use tempfile::tempdir;
    /// # let dir = tempdir()?;
    /// # let path = dir.path().join("mysocket");
    ///
    /// // let path = ...
    /// let listener = UnixListener::bind(&path)?;
    /// # thread::spawn(move || UnixStream::connect(path).expect("Can't connect"));
    ///
    /// let mut incoming = listener.incoming();
    ///
    /// let sock = match incoming.next() {
    ///     Some(Ok(sock)) => sock,
    ///     Some(Err(e)) => {
    ///         println!("Can't get the next incoming socket: {}", e);
    ///         return Ok(());
    ///     }
    ///     None => unreachable!(),
    /// };
    ///
    /// # Ok::<(),std::io::Error>(())
    /// ```
    ///
    /// [SocketAddr]: https://doc.rust-lang.org/stable/std/os/unix/net/struct.SocketAddr.html
    pub fn incoming(&self) -> Incoming {
        Incoming {
            listener: self,
        }
    }
}

impl AsRawFd for UnixListener {
    fn as_raw_fd(&self) -> RawFd {
        self.inner.as_raw_fd()
    }
}

impl FromRawFd for UnixListener {
    unsafe fn from_raw_fd(fd: RawFd) -> Self {
        StdUnixListner::from_raw_fd(fd).into()
    }
}

impl IntoRawFd for UnixListener {
    fn into_raw_fd(self) -> RawFd {
        self.inner.into_raw_fd()
    }
}

impl<'a> IntoIterator for &'a UnixListener {
    type Item = io::Result<UnixStream>;
    type IntoIter= Incoming<'a>;

    fn into_iter(self) -> Self::IntoIter {
        self.incoming()
    }
}

impl From<StdUnixListner> for UnixListener {
    fn from(inner: StdUnixListner) -> Self {
        UnixListener { inner }
    }
}

// === impl Incoming ===
impl Iterator for Incoming<'_> {
    type Item = io::Result<UnixStream>;

    fn next(&mut self) -> Option<Self::Item> {
        Some(self.listener.accept().map(|(s, _)| s))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (usize::MAX, None)
    }
}

// === impl CMsgTruncatedError ===
impl CMsgTruncatedError {
    fn new() -> CMsgTruncatedError {
        CMsgTruncatedError { }
    }
}

impl fmt::Display for CMsgTruncatedError {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "The buffer used to receive file descriptors was too small.")
    }
}

impl std::error::Error for CMsgTruncatedError { }

#[cfg(test)]
mod test {
    use super::*;

    use std::convert::AsMut;
    use std::ffi::c_void;
    use std::ptr;
    use std::slice;

    use nix::fcntl::OFlag;
    use nix::sys::mman::{mmap, munmap, shm_open, shm_unlink, ProtFlags, MapFlags};
    use nix::sys::stat::Mode;
    use nix::unistd::{close, ftruncate};

    struct Shm {
        fd: RawFd,
        ptr: *mut u8,
        len: usize,
        name: String,
    }

    impl Shm {
        fn new(name: &str, size: i64) -> Shm {
            let oflag = OFlag::O_CREAT | OFlag::O_RDWR;
            let fd = shm_open(name, oflag, Mode::S_IRUSR | Mode::S_IWUSR)
                .expect("Can't create shm.");
            ftruncate(fd, size).expect("Can't ftruncate");
            let len: usize = size as usize;

            let prot = ProtFlags::PROT_READ | ProtFlags::PROT_WRITE;
            let flags = MapFlags::MAP_SHARED;

            let ptr = unsafe {
                mmap(ptr::null_mut(), len, prot, flags, fd, 0).expect("Can't mmap") as *mut u8
            };

            Shm{ fd, ptr, len, name: name.to_string() }
        }

        fn from_raw_fd(fd: RawFd, size: usize) -> Shm {
            let prot = ProtFlags::PROT_READ | ProtFlags::PROT_WRITE;
            let flags = MapFlags::MAP_SHARED;

            let ptr = unsafe {
                mmap(ptr::null_mut(), size, prot, flags, fd, 0). expect("Can't mmap") as *mut u8
            };

            Shm{ fd, ptr, len: size, name: String::new() }
        }
    }

    impl Drop for Shm {
        fn drop(&mut self) {
            unsafe {
                munmap(self.ptr as *mut c_void, self.len).expect("Can't munmap");
            }
            close(self.fd).expect("Can't close");
            if !self.name.is_empty() {
                let name: &str = self.name.as_ref();
                shm_unlink(name).expect("Can't shm_unlink");
            }
        }
    }

    impl AsMut<[u8]> for Shm {
        fn as_mut(&mut self) -> &mut [u8] {
            unsafe {
                slice::from_raw_parts_mut(self.ptr, self.len)
            }
        }
    }

    impl AsRawFd for Shm {
        fn as_raw_fd(&self) -> RawFd {
            self.fd
        }
    }

    fn make_hello(name: &str) -> Shm {
        let hello = b"Hello World!\0";
        let mut shm = Shm::new(name, hello.len() as i64);

        shm.as_mut().copy_from_slice(hello.as_ref());

        shm
    }

    fn compare_hello(fd: RawFd) -> bool {
        let hello = b"Hello World!\0";
        let mut shm = Shm::from_raw_fd(fd, hello.len());

        &shm.as_mut()[..hello.len()] == hello.as_ref()
    }

    #[test]
    fn unix_stream_passes_fd() {
        let shm = make_hello("/unix_stream_passes_fd");
        let mut buf = vec![0; 20];

        let (mut sut1, mut sut2) = UnixStream::pair().expect("Can't make pair");
        sut1.enqueue(&shm).expect("Can't enqueue");
        sut1.write(b"abc").expect("Can't write");
        sut1.flush().expect("Can't flush");
        sut2.read(&mut buf).expect("Can't read");
        let fd = sut2.dequeue().expect("Empty fd queue");

        assert!(fd != shm.fd, "fd's unexpectedly equal");
        assert!(compare_hello(fd), "fd didn't contain expect contents");
    }
}
