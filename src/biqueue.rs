// Copyright 2022 Steven Bosnick
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE-2.0 or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms

//! Bi-directional fd queue with the ability to ability to pass the fd's over
//! a provided unix stream fd.

use std::{
    collections::VecDeque,
    fmt,
    io::{self, Error, ErrorKind, IoSlice, IoSliceMut},
    os::unix::io::{AsRawFd, IntoRawFd, RawFd},
};

use ::tracing::{trace, warn};

use crate::{DequeueFd, EnqueueFd, QueueFullError};
use iomsg::{cmsg_buffer_fds_space, Fd, MsgHdr};

mod iomsg;

/// The Bi-directional queue for fd passing.
///
/// A `BiQueue` consists of an inbound fd queue and an outbound fd queue that
/// will pass the queued fd's over a unix stream in the [`BiQueue::write_vectored`]
/// and [`BiQueue::read_vectored`] methods. The inbound and outbound queues are accessed
/// through the [`EnqueueFd`] and [`DequeueFd`] trait impl's.
#[derive(Debug)]
pub struct BiQueue {
    infd: VecDeque<Fd>,
    outfd: Vec<RawFd>,
}

#[derive(Debug)]
struct CMsgTruncatedError {}

#[derive(Debug)]
struct PushFailureError {}

trait Push<A> {
    fn push(&mut self, item: A) -> Result<(), A>;
}

// === impl Biqueue ===

impl BiQueue {
    pub const FD_QUEUE_SIZE: usize = 2;

    pub fn new() -> Self {
        BiQueue {
            infd: VecDeque::with_capacity(Self::FD_QUEUE_SIZE),
            outfd: Vec::with_capacity(Self::FD_QUEUE_SIZE),
        }
    }

    pub fn write_vectored(&mut self, fd: impl AsRawFd, bufs: &[IoSlice]) -> io::Result<usize> {
        send_fds(fd.as_raw_fd(), bufs, self.outfd.drain(..))
    }

    pub fn read_vectored(
        &mut self,
        fd: impl AsRawFd,
        bufs: &mut [IoSliceMut],
    ) -> io::Result<usize> {
        recv_fds(fd.as_raw_fd(), bufs, self)
    }
}

impl DequeueFd for BiQueue {
    fn dequeue(&mut self) -> Option<RawFd> {
        let result = self.infd.pop_front().map(|fd| fd.into_raw_fd());

        trace!(
            source = "UnixStream",
            event = "dequeue",
            count = if result.is_some() { 1 } else { 0 }
        );

        result
    }
}

impl Push<Fd> for BiQueue {
    fn push(&mut self, item: Fd) -> Result<(), Fd> {
        self.infd.push_back(item);
        Ok(())
    }
}

impl EnqueueFd for BiQueue {
    fn enqueue(&mut self, fd: &impl AsRawFd) -> std::result::Result<(), QueueFullError> {
        if self.outfd.len() >= Self::FD_QUEUE_SIZE {
            warn!(source = "UnixStream", event = "enqueue", condition = "full");
            Err(QueueFullError::new())
        } else {
            self.outfd.push(fd.as_raw_fd());
            trace!(source = "UnixStream", event = "enqueue", count = 1);
            Ok(())
        }
    }
}

// === helper functions ===

fn send_fds(
    sockfd: RawFd,
    bufs: &[IoSlice],
    fds: impl Iterator<Item = RawFd>,
) -> io::Result<usize> {
    // Size the buffer to be big enough to hold FD_QUEUE_SIZE RawFd's.
    // The buffer must be zeroed because subsequent code will not clear padding
    // bytes.
    let mut cmsg_buffer = [0u8; cmsg_buffer_fds_space(BiQueue::FD_QUEUE_SIZE)];

    let counts = MsgHdr::from_io_slice(bufs, &mut cmsg_buffer)
        .encode_fds(fds)?
        .send(sockfd)?;

    trace!(
        source = "UnixStream",
        event = "write",
        fds_count = counts.fds_sent(),
        byte_count = counts.bytes_sent(),
    );

    Ok(counts.bytes_sent())
}

fn recv_fds(
    sockfd: RawFd,
    bufs: &mut [IoSliceMut],
    fds_sink: &mut impl Push<Fd>,
) -> io::Result<usize> {
    // Size the buffer to be big enough to hold FD_QUEUE_SIZE RawFd's.
    let mut cmsg_buffer = [0u8; cmsg_buffer_fds_space(BiQueue::FD_QUEUE_SIZE)];

    let mut recv = MsgHdr::from_io_slice_mut(bufs, &mut cmsg_buffer).recv(sockfd)?;

    let mut fds_count = 0;
    for fd in recv.take_fds() {
        match fds_sink.push(fd) {
            Ok(_) => fds_count += 1,
            Err(_) => {
                warn!(
                    source = "UnixStream",
                    event = "read",
                    condition = "too many fds received"
                );

                return Err(PushFailureError::new());
            }
        }
    }

    if recv.was_control_truncated() {
        warn!(
            source = "UnixStream",
            event = "read",
            condition = "cmsgs truncated"
        );

        Err(CMsgTruncatedError::new())
    } else {
        trace!(
            source = "UnixStream",
            event = "read",
            fds_count,
            byte_count = recv.bytes_recvieved(),
        );

        Ok(recv.bytes_recvieved())
    }
}

// === impl CMsgTruncatedError ===
impl CMsgTruncatedError {
    fn new() -> Error {
        Error::new(ErrorKind::Other, CMsgTruncatedError {})
    }
}

impl fmt::Display for CMsgTruncatedError {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(
            f,
            "The buffer used to receive file descriptors was too small."
        )
    }
}

impl std::error::Error for CMsgTruncatedError {}

// === impl PushFailureError ===
impl PushFailureError {
    fn new() -> Error {
        Error::new(ErrorKind::Other, PushFailureError {})
    }
}

impl fmt::Display for PushFailureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "The sink for receiving file descriptors was unexpectedly full."
        )
    }
}

impl std::error::Error for PushFailureError {}
