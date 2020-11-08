// Copyright 2020 Steven Bosnick
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE-2.0 or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms

use std::{
    os::unix::io::RawFd,
    mem,
    io::Write,
    env::var_os, path::Path, fs::File, io::BufWriter};

use libc::c_uint;

const MAX_FD_COUNT: usize = 10;

fn main() {
    // Safety: CMSG_SPACE is safe
    let scm_rights_space = unsafe {libc::CMSG_SPACE((MAX_FD_COUNT * mem::size_of::<RawFd>()) as c_uint)};

    let path = var_os("OUT_DIR")
        .map(|s| Path::new(&s).join("constants.rs"))
        .expect("Can't find OUT_DIR");
    let mut output = File::create(path)
        .map(BufWriter::new)
        .expect("Can't create constants.rs file");

    write!(output, "/// the result of `libc::CMSG_SPACE()` for `MAX_FD_COUNT` `RawFd`'s\n\
                    pub const CMSG_SCM_RIGHTS_SPACE: libc::c_uint = {};\n\
                    \n\
                    /// the maximum number of `RawFd`'s to transfer in a single call to \n\
                    /// `libc::sendmsg` or `libc::recvmsg`.\n\
                    pub const MAX_FD_COUNT: usize = {};\n", scm_rights_space, MAX_FD_COUNT)
        .expect("Can't write to constants.rs");
}
