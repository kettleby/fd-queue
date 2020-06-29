// Copyright 2020 Steven Bosnick
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE-2.0 or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms

use std::error::Error;
use std::fmt::{self, Display};
use std::marker::PhantomData;
use std::os::unix::io::{AsRawFd, RawFd};

/// An interface to enqueue a [`RawFd`][RawFd] for later tranmission to a different
/// process.
///
/// This trait is intended to interact with [`Write`][Write] as the mechanism for
/// actually transmitting the enqueued [`RawFd`][RawFd]. Specfically, the `RawFd`
/// will be transmittied after a `write()` of at least 1 byte and, possibly, a
/// `flush()`.
///
/// [RawFd]: https://doc.rust-lang.org/stable/std/os/unix/io/type.RawFd.html
/// [Write]: https://doc.rust-lang.org/stable/std/io/trait.Write.html
pub trait EnqueueFd {
    /// Enqueue `fd` for later tranmission to a different process.
    ///
    /// The caller is responsible for keeping `fd` open until after the `write()` and
    /// `flush()` calls for actually transmitting the `fd` have been completed.
    fn enqueue(&mut self, fd: &impl AsRawFd) -> Result<(), QueueFullError>;
}

/// An interface to dequeue a [`RawFd`][RawFd] that was previously transmitted from a
/// different process.
///
/// This trait is intended to interact with [`Read`][Read] as the mechanism for
/// actually tranmitting the [`RawFd`][RawFd] before it can be dequeued. Specfically
/// the `RawFd` will become available for dequeuing after at least 1 byte has been
/// `read()`.
///
/// [RawFd]: https://doc.rust-lang.org/stable/std/os/unix/io/type.RawFd.html
/// [Read]: https://doc.rust-lang.org/stable/std/io/trait.Read.html
pub trait DequeueFd {
    /// Dequeue a previouly transmitted [`RawFd`][RawFd].
    ///
    /// The caller is responsible for closing this `RawFd`.
    ///
    /// [RawFd]: https://doc.rust-lang.org/stable/std/os/unix/io/type.RawFd.html
    fn dequeue(&mut self) -> Option<RawFd>;
}

/// Error returned when the queue of [`RawFd`][RawFd] is full.
///
/// [RawFd]: https://doc.rust-lang.org/stable/std/os/unix/io/type.RawFd.html
#[derive(Debug, Default)]
pub struct QueueFullError {
    _private: PhantomData<()>,
}

impl QueueFullError {
    /// Create a new `QueueFullError`.
    #[inline]
    pub fn new() -> QueueFullError {
        QueueFullError {
            _private: PhantomData,
        }
    }
}

impl Display for QueueFullError {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "file descriptor queue is full")
    }
}

impl Error for QueueFullError {}
