# FD Queue

**FD Queue is a Rust abstraction for passing file descriptors between processes.**

[![CI](https://github.com/kettleby/fd-queue/workflows/CI/badge.svg)](https://github.com/kettleby/fd-queue/actions?query=workflow%3ACI)
[![doc](https://docs.rs/fd-queue/badge.svg)](https://docs.rs/fd-queue)
[![Crates.io](https://img.shields.io/crates/v/fd-queue)](https://crates.io/crates/fd-queue)
[![Release](https://img.shields.io/github/v/release/kettleby/fd-queue?include_prereleases&sort=semver)](https://github.com/kettleby/fd-queue/releases)
---

fd-queue provides traits for enqueuing and dequeuing file descriptors and
implementations of those traits for different types of Unix sockets.
Specifically fd-queue provides a blocking implementation, a non-blocking
implementation base on [mio], and a non-blocking implementation based on
[tokio].

[mio]: https://crates.io/crates/mio
[tokio]: https://crates.io/crates/tokio

## Usage

Add this to your `Cargo.toml`

```toml
[dependencies]
fd-queue = {version = "1.0.0", features = ["net-fd"]}
```

This enables the blocking implementation of the traits for enqueuing and
dequeuing file descriptors. See below for the other features. You can then
use the library as follows:

```rust
use std::{
    fs::File,
    io::prelude::*,
    os::unix::io::FromRawFd,
};
use fd_queue::{EnqueueFd, DequeueFd, UnixStream};

let (mut sock1, mut sock2) = UnixStream::pair()?;

// sender side
let file: File = ...
sock1.enqueue(&file).expect("Can't enquque the file descriptor.");
sock1.write(b"a")?;
sock1.flush()?;

//receiver side
let mut buf = [0u8; 1];
sock2.read(&mut buf)?;
let fd = sock2.dequeue().expect("Can't dequeue the file descriptor.");
let file2 = unsafe { File::from_raw_fd(fd) };
```

## Features
Usage of the library with the default features will include only the basic
trait definitions `DequeueFd` and `EnqueueFd` together with their supporting
types. With the default features there will be no implementations of the basic
traits. To include implementations of the traits enable the following features:

| Feature  | Implementation | Additional Traits          |
|----------|----------------|----------------------------|
| net-fd   | blocking       | `Read`, `Write`            |
| mio-fd   | non-blocking   | `Read`, `Write`, `Evented` |
| tokio-fd | non-blocking   | `AsyncRead`, `AsyncWrite`  |

## Rust Version Requirements
The library will always support the Rust version that is two earlier
than the current stable version. The current Minimum Supported Rust
Version (MSRV) is 1.61.0. Any change to the MSRV will be treated as a
minor change for Semantic Version purposes.

## Semantic Version and Release
This library follows [semantic versioning][semver].

The first non-pre-release version of the library will be version 1.0.0. This
does not signal that the library is production ready or that we will attempt
to avoid breaking changes. It rather signals exactly what the [Semantic Versioning
Specification][semver] says it does: there won't be any backward incompatible
changes until version 2.0.0 (see [here][semver-8]).

For a signal of the maturity of the library see the next heading which will be
updated as the library matures.

[semver]: https://semver.org/
[semver-8]: https://semver.org/#spec-item-8

## Maturity
This library is an initial, experimental implementation that has not had any
use in production. You should expect breaking changes (with an appropriate change
in [semantic version][semver]) as the library matures.

## License

FD Queue is licensed under either of

 * Apache License, Version 2.0, ([LICENSE-APACHE-2.0](LICENSE-APACHE-2.0) or
   http://www.apache.org/licenses/LICENSE-2.0)
 * MIT license ([LICENSE-MIT](LICENSE-MIT) or
   http://opensource.org/licenses/MIT)

at your option.

## Contribution

Please note that this project is released with a [Contributor Code of
Conduct][code-of-conduct].  By participating in this project you agree to abide
by its terms.

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in FD Queue by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.

[code-of-conduct]: CODE_OF_CONDUCT.md
