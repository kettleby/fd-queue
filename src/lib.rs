// Copyright 2020 Steven Bosnick
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE-2.0 or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms

//! A library for providing abstractions for passing [`RawFd`][RawFd] between
//! processes. This is necessarily a Unix (or Unix-like) library as `RawFd` are Unix
//! specific.
//!
//! The underlying mechanism for passing the `RawFd` is a Unix socket, but the
//! different abstractions provided here are different ways of embedding this in the
//! Rust ecosystem.
//!
//! [RawFd]: https://doc.rust-lang.org/stable/std/os/unix/io/type.RawFd.html

#![deny(missing_docs, warnings)]

mod queue;

#[cfg(any(feature = "net-fd", feature = "tokio-fd"))]
mod biqueue;

#[cfg(feature = "net-fd")]
mod net;

#[cfg(feature = "mio-fd")]
pub mod mio;

#[cfg(feature = "tokio-fd")]
pub mod tokio;

#[cfg(feature = "net-fd")]
pub use net::{Incoming, UnixListener, UnixStream};

pub use queue::{DequeueFd, EnqueueFd, QueueFullError};
