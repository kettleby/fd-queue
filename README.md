# FD Queue

**FD Queue is a Rust abstraction for passing file descriptors between processes.**

[![CI](https://github.com/sbosnick/fd-queue/workflows/CI/badge.svg)](https://github.com/sbosnick/fd-queue/actions?query=workflow%3ACI)
---


fd-queue provides traits for enqueuing and dequeuing file descriptors and
implementations of those traits for different types of unix sockets.
Specifically fd-queue provides a blocking implementation, a non-blocking
implementation base on [mio], and a non-blocking implementation based on
[tokio].

[mio]: https://crates.io/crates/mio
[tokio]: https://crates.io/crates/tokio

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
