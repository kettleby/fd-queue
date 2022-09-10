# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [1.1.1] - 2022-09-10

### Fixed
- *(net)* change libc dependencies to allow build (#49)

### Other
- enable release workflow for sbosnick-bot (#48)
- update pin-project dependency (#46)
- change release workflow to use GH PAT (#43)

## [1.1.0] - 2022-08-21

### Added
- *(tokio)* change UnixStream to not use AsyncFd (#38)

### Other
- fix various spelling error in doc comments (#41)
- update README for 1.0.0 release (#40)
- fix release workflow (#39)
- fix release workflow syntax error
- convert release workflow to release-plz
- update version of the checkout action
- *(README)* remove references to semantic-release

## [1.0.0] - 2022-07-18

### Added
- *(ci)* [**breaking**] increase MSRV to 1.49.0
- *(ci)* increase the MRSV
- *(release)* add information for initial (pre) release
- *(tokio)* Implement UnixListener for tokio integration
- *(tokio)* Implement UnixStream for tokio integration
- *(tokio)* add skeleton for tokio support
- *(queue)* add Default impl for QueueFullError
- *(mio)* add mio support for non-blocking UnixListener
- *(mio)* add mio support for non-blocking UnixStream

### Fixed
- *(tokio)* upgrade to tokio 1.0.0 (#26)
- *(release)* update semantic-release-rust version
- *(release)* correct stored credentials error
- *(release)* correct docs.rs metadata
- *(release)* correct inter-artifact links
- *(release)* change process for committing version number
- remove default features
- *(tokio)* change futures dependencies to require tokio-fd feature
- *(tokio)* change UnixStream::connect to be async
- *(mio)* downgrade mio dependancy to 0.6.22
- *(mio)* fix build error for MSRV of 1.36.0

### Other
- *(devenv)* add release-plz to the devenv
- *(ci)* upgrade the release plugin
- *(direnv)* nixify the development environment
- *(release)* 1.0.0-beta.3 [skip ci]
- *(net)* add tests for some edge conditions in iomsg (#25)
- *(net)* remove nix dependency (#24)
- *(release)* fix version of semantic-release-rust (#23)
- change install of semantic-release-rust (#22)
- *(release)* change source for semantic-release-rust (#21)
- add audit check workflow (#20)
- *(release)* 1.0.0-beta.2 [skip ci]
- add beta branch to workflow
- *(release)* 1.0.0-beta.1 [skip ci]
- *(release)* add beta branch to workflow
- *(release)* 1.0.0-alpha.4 [skip ci]
- *(release)* 1.0.0-alpha.3 [skip ci]
- *(release)* 1.0.0-alpha.2 [skip ci]
- *(release)* version 1.0.0-alpha.1
- *(release)* change condition for merge PR step
- *(release)* change install of semantic-release-rust
- fix merge pull request step in release workflow
- enhance release workflow
- split CI workflow and the Release workflow
- *(CI)* correct error with semantic release job
- *(CI)* correct error in CI workflow file
- *(CI)* add initial semantic-release configuration
- fix typo in the CI workflow
- fix typo in the CI workflow file
- add step to CI workflow for MSRV
- Increase MSRV in the CI workflow
- *(tokio)* run cargo fmt
- *(ci)* add rustfmt and clippy checks to the CI workflow
- add net-fd feature to make its dependencies optional
- run cargo fmt
- Correct reference to other project in README.md
- Refactor root module
- Fix error in the use of usize::MAX in Incoming
- Fix error in the used of isize::MAX in UnixStream
- Add tracing to key UnixStream methods
- Audit the use of unsafe in UnixStream
- Extend the documentation of UnixListener
- Extend the documentation of UnixStream
- Improve documentation
- Implement UnixListener
- Add UnixStream with EnqueueFd and DequeueFd
- Add EnqueueFd and DequeueFd traits
- Add CI badge to README.md
- Extend initial commit
- Initial commit
