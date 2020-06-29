// Copyright 2020 Steven Bosnick
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE-2.0 or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms

//! An implementation of `EnqueueFd` and `DequeueFd` that is integrated with tokio.

/// A structure representing a connected Unix socket.
///
/// This socket can be connected directly with UnixStream::connect or accepted from a listener with
/// UnixListener::incoming. Additionally, a pair of anonymous Unix sockets can be created with
/// UnixStream::pair.
pub struct UnixStream {}

/// A Unix socket which can accept connections from other Unix sockets.
///
/// You can accept a new connection by using the accept method. Alternatively UnixListener
/// implements the Stream trait, which allows you to use the listener in places that want a stream.
/// The stream will never return None and will also not yield the peer's SocketAddr structure.
/// Iterating over it is equivalent to calling accept in a loop.
pub struct UnixListener {}

// === impl UnixStream ===
// TODO: implement UnixStream

// === impl UnixListener ===
// TODO: implment UnixListener
