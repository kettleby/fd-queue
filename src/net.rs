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
use std::marker::PhantomData;
use std::mem::size_of;
use std::net::Shutdown;
use std::os::unix::io::{AsRawFd, FromRawFd, IntoRawFd, RawFd};
use std::os::unix::net::{SocketAddr, UnixStream as StdUnixStream};
use std::path::Path;
use std::slice;

use nix::cmsg_space;
use nix::sys::socket::{recvmsg, sendmsg, ControlMessage, ControlMessageOwned, MsgFlags};
use nix::sys::uio::IoVec;

use crate::{EnqueueFd, DequeueFd, QueueFullError};

/// A structure representing a connected Unix socket with support for passing
/// [`RawFd`][RawFd].
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
pub struct UnixListener {}

#[derive(Debug)]
struct CMsgTruncatedError {
    _private: PhantomData<()>,
}

// === impl UnixStream ===
impl UnixStream {

    /// The size of the bounded queue of outbound [`RawFd`][RawFd].
    ///
    /// [RawFd]: https://doc.rust-lang.org/stable/std/os/unix/io/type.RawFd.html
    pub const FD_QUEUE_SIZE: usize = 2;

    /// Connects to the socket named by `path`.
    pub fn connect<P: AsRef<Path>>(path: P) -> io::Result<UnixStream> {
        StdUnixStream::connect(path).map(|s| s.into())
    }

    /// Creates an unamed pair of connected sockets.
    ///
    /// Returns two `UnixStream`s which are connected to each other.
    pub fn pair() -> io::Result<(UnixStream, UnixStream)> {
        StdUnixStream::pair().map(|(s1, s2)| (s1.into(), s2.into()))
    }

    /// Creates a new independently owned handle to the underlying socket.
    ///
    /// The returned `UnixStream` is a reference to the same stream that this object references.
    /// Both handles will read and write the same stream of data, and options set on one stream
    /// will be propagated to the other stream.
    pub fn try_clone(&self) -> io::Result<UnixStream> {
        self.inner.try_clone().map(|s| s.into())
    }

    /// Returns the socket address of the local half of this connection.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.inner.local_addr()
    }

    /// Returns the socket address of the remote half of this connection.
    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        self.inner.peer_addr()
    }

    /// Returns the value of the `SO_ERROR` option.
    pub fn take_error(&self) -> io::Result<Option<Error>> {
        self.inner.take_error()
    }

    /// Shuts down the read, write, or both halves of this connection.
    ///
    /// This function will cause all pending and future I/O calls on the specified portions to
    /// immediately return with an appropriate value.
    pub fn shutdown(&self, how: Shutdown) -> io::Result<()> {
        self.inner.shutdown(how)
    }
}

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

impl DequeueFd for UnixStream {
    fn dequeue(&mut self) -> Option<RawFd> {
        self.infd.pop_front()
    }
}

impl Read for UnixStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.read_vectored(&mut [IoSliceMut::new(buf)])
    }

    fn read_vectored(&mut self, bufs: &mut [IoSliceMut]) -> io::Result<usize> {
        assert_eq!(size_of::<IoSliceMut>(), size_of::<IoVec<&mut [u8]>>());

        let bufs_ptr = bufs.as_mut_ptr();
        let vecs_ptr = bufs_ptr as *mut IoVec<&mut [u8]>;
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

impl Write for UnixStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.write_vectored(&[IoSlice::new(buf)])
    }

    fn write_vectored(&mut self, bufs: &[IoSlice]) -> io::Result<usize> {
        assert_eq!(size_of::<IoSlice>(), size_of::<IoVec<&[u8]>>());

        let bufs_ptr = bufs.as_ptr();
        let vecs_ptr = bufs_ptr as *const IoVec<&[u8]>;
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


// === impl CMsgTruncatedError ===

impl CMsgTruncatedError {
    fn new() -> CMsgTruncatedError {
        CMsgTruncatedError {
            _private: PhantomData,
        }
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

        assert!(fd != shm.fd, "fd's unexpectly equal");
        assert!(compare_hello(fd), "fd didn't contain expect contents");
    }
}
