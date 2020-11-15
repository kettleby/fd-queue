// Copyright 2020 Steven Bosnick
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE-2.0 or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms

//! Types to provide a safe interface around libc::recvmsg and libc::sendmsg.

use std::{
    io,
    io::{IoSlice, IoSliceMut},
    marker::PhantomData,
    mem,
    ops::Neg,
    os::unix::io::RawFd,
    ptr,
};

use libc::{
    cmsghdr, iovec, msghdr, recvmsg, CMSG_DATA, CMSG_FIRSTHDR, CMSG_NXTHDR, CMSG_SPACE, MSG_CTRUNC,
    SCM_RIGHTS, SOL_SOCKET,
};
use num_traits::One;

#[derive(Debug)]
pub struct MsgHdr<'a, State> {
    // Invariant: mhdr is properly initalized with msg_name null, and with
    // msg_iov and msg_control valid pointers for length msg_iovlen and
    // msg_controllen respectively. The arrays that msg_iov and msg_control
    // point to must outlive mhdr.
    mhdr: msghdr,
    state: State,
    _phantom: PhantomData<&'a ()>,
}

// The type states fro MsgHdr.
#[derive(Debug, Default)]
pub struct RecvStart {}

#[derive(Debug)]
pub struct RecvEnd {
    bytes_recvieved: usize,
}

#[derive(Debug, Default)]
pub struct SendStart {}

struct FdsIter<'a> {
    // Invariant: mhdr is initalized as described in MsgHdr.that has
    // been filled in by a call to recvmsg.
    mhdr: &'a msghdr,
    // Invariant: cmsg is a valid cmsg based on mhdr (or None)
    cmsg: Option<&'a cmsghdr>,
    data: Option<FdsIterData>,
}

// Invariants:
//      1. curr and end are non-null
//      2. curr <= end
//      3. the half-open range [curr, end) (if non-empty) must
//          be all part of the same same array of initalized
//          RawFd values.
//      4. if curr != end then curr must be a valid pointer
//  Note that neither curr nor end is assumed to be ali_gned.
struct FdsIterData {
    curr: *const RawFd,
    end: *const RawFd,
}

impl<'a, State: Default> MsgHdr<'a, State> {
    // Safety: iov must be valid for length iov_len and the array that iov points to
    // must outlive the returned MsgHdr.
    unsafe fn new(iov: *mut iovec, iov_len: usize, cmsg_buffer: &'a mut [u8]) -> Self {
        let mhdr = {
            // msghdr may have private members for padding. This ensures that they are zeroed.
            let mut mhdr = mem::MaybeUninit::<libc::msghdr>::zeroed();
            // Safety: we don't turn p into a reference or read from it
            let p = mhdr.as_mut_ptr();
            (*p).msg_name = ptr::null_mut();
            (*p).msg_namelen = 0;
            (*p).msg_iov = iov;
            (*p).msg_iovlen = iov_len;
            (*p).msg_control = cmsg_buffer.as_mut_ptr() as _;
            (*p).msg_controllen = cmsg_buffer.len();
            (*p).msg_flags = 0;
            // Safety: we have initalized all 7 fields of mhdr and zeroed the padding
            mhdr.assume_init()
        };

        // Invariant: msg_name is set to null; msg_control (and its len) are
        // set based on a slice that outlives mhdr; the requirements on
        // msg_iov (and its len) are precondition to this function.
        Self {
            mhdr,
            state: Default::default(),
            _phantom: PhantomData,
        }
    }
}

impl<'a> MsgHdr<'a, RecvStart> {
    /// Returns the size needed for a msghdr control buffer big
    /// enough to hold `count` `RawFd`'s.
    #[allow(dead_code)]
    pub fn cmsg_buffer_fds_space(count: usize) -> usize {
        // Safety: CMSG_SPACE is safe
        unsafe { CMSG_SPACE((count * mem::size_of::<RawFd>()) as u32) as usize }
    }

    pub fn from_io_slice_mut(bufs: &'a mut [IoSliceMut], cmsg_buffer: &'a mut [u8]) -> Self {
        // IoSliceMut guarentees ABI compatibility with iovec.
        let iov: *mut iovec = bufs.as_mut_ptr() as *mut iovec;
        let iov_len = bufs.len();

        // Safety: iov is valid for iov_len because they both come from the same
        // slice (bufs). The array that iov points to will outlive the returned
        // MsgHdr because of the lifetime constraints on bufs and on MsgHdr.
        unsafe { Self::new(iov, iov_len, cmsg_buffer) }
    }

    pub fn recv(mut self, sockfd: RawFd) -> io::Result<MsgHdr<'a, RecvEnd>> {
        // Safety: the invariant on self.mhdr mean that it has been properly
        // initalized for passing to recvmsg.
        let count =
            call_res(|| unsafe { recvmsg(sockfd, &mut self.mhdr, 0) }).map(|c| c as usize)?;

        // Invariant: self.mhdr satified the invariant at the start of this call.
        // recvmsg can write into the buffers pointed to by the iovec's found
        // in the array pointed to by mhdr.iov but won't change the array itself.
        // recvmsg may change msg_controllen to the length of the actual control
        // buffer read, but this will be no longer than the msg_controllen passed
        // in so msg_control will still be a valid pointer for length
        // msg_controllen.
        Ok(MsgHdr {
            mhdr: self.mhdr,
            state: RecvEnd {
                bytes_recvieved: count,
            },
            _phantom: PhantomData,
        })
    }
}

impl<'a> MsgHdr<'a, RecvEnd> {
    pub fn bytes_recvieved(&self) -> usize {
        self.state.bytes_recvieved
    }

    pub fn was_control_truncated(&self) -> bool {
        self.mhdr.msg_flags & MSG_CTRUNC != 0
    }

    pub fn fds_iter(&'a self) -> impl Iterator<Item = RawFd> + 'a {
        // Safety: the invariant on self.mhdr means it is initalized
        // appropriately. The trasition from RecvStart state to RecvEnd
        // state means that recvmsg was called. The appropriate lifetimes
        // for the buffers in mhdr are guarenteed by holding a shared
        // reference to self and by the absence of other methods that
        // can modify a MsgHdr<RecvEnd>.
        unsafe { FdsIter::new(&self.mhdr) }
    }
}

impl<'a> MsgHdr<'a, SendStart> {
    // TODO: remove this when it is not longer necessary
    #[allow(dead_code)]
    pub fn from_io_slice(bufs: &'a [IoSlice], cmsg_buffer: &'a mut [u8]) -> Self {
        // IoSlice guarentees ABI compatibility with iovec. sendmsg doesn't
        // mutate the iovec array but the standard says it takes a mutable
        // pointer so this transmutes he pointer into a mutable one. The Role
        // constraint of Sender on MsgHdr will prevent calling recvmsg on the
        // underlying msghdr.
        let iov: *mut iovec = bufs.as_ptr() as *mut iovec;
        let iov_len = bufs.len();

        // Safety: iov is valid for iov_len because they both come from the same
        // slice (bufs). The array that iov points to will outlive the returned
        // MsgHdr because of the lifetime constraints on bufs and on MsgHdr.
        unsafe { Self::new(iov, iov_len, cmsg_buffer) }
    }
}

impl<'a> FdsIter<'a> {
    // Safety: mhdr is initalized as described in invariant for MsgHdr and
    // has been filled in by a call to recvmsg. The buffers pointed to
    // by fields of mhdr must not be changed during the lifetime of the
    // returned reference.
    unsafe fn new(mhdr: &'a msghdr) -> Self {
        // Safety: follows from pre-condition.
        let cmsg = FdsIter::first_cmsg(mhdr);
        // Safety: follows from pre-condition (especially the call to recvmsg).
        let data = cmsg.and_then(|cmsg| FdsIterData::new(cmsg));

        FdsIter {
            // Invariant: follows from pre-condition.
            mhdr,
            // Invariant: cmsg is produced by first_cmsg based on mhdr
            cmsg,
            data,
        }
    }

    // Safety: mhdr is initalized as described in invariant for MsgHdr and
    // has been filled in by a call to recvmsg. The buffers pointed to
    // by fields of mhdr must not be changed during the lifetime of the
    // returned reference.
    unsafe fn first_cmsg(mhdr: &'a msghdr) -> Option<&'a cmsghdr> {
        // Safety: follows from pre-condition.
        let cmsg = CMSG_FIRSTHDR(mhdr);
        // Safety: if CMSG_FIRSTHDR returns a non-null pointer it will be a
        // properly aligned pointer to a cmsghdr. It also guarentees that it
        // points to a part of the msg_control that is big enough for a cmsghdr
        // which means that the pointer is dereferenceable. Finally, the
        // precondition on not changing the buffers of the fields pointed to by
        // mhdr and the lifetime constraint on the functions return value
        // enforce Rust's aliasing rules.
        cmsg.as_ref()
    }

    fn advance_cmsg(&mut self) {
        if let Some(cmsg) = self.cmsg {
            // Safety: cmsg is a valid cmsg based on mhdr which has a
            // msg_control array filled in by recvmsg (from the invariants).
            // The call to CMSG_NXTHDR is thus safe by its defintion. If
            // CMSG_NXTHDR returns a non-null pointer it will be a properly
            // alligned pointer to a cmsghdr. It also points to a part of the
            // msg_control that is big enough for a cmsghdr which means that
            // the pointer is dereferenceable. Finally, the precondition on not
            // changing the buffers of the fields pointed to by mhdr and the
            // lifetime constraint on the functions return value enforce Rust's
            // aliasing rules.
            let new_cmsg = unsafe { CMSG_NXTHDR(self.mhdr, cmsg).as_ref() };

            // Safety: follows from the invariant on msghdr and especially
            // the requirement to have called recvmsg.
            let new_data = new_cmsg.and_then(|cmsg| unsafe { FdsIterData::new(cmsg) });

            // Invariant: new_cmsg is produced by a call to CMSG_NXTHDR.
            self.cmsg = new_cmsg;
            self.data = new_data;
        }
    }
}

impl<'a> Iterator for FdsIter<'a> {
    type Item = RawFd;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            match self.data.as_mut().and_then(|data| data.next()) {
                Some(data) => return Some(data),
                None => {
                    self.advance_cmsg();
                    if self.cmsg.is_none() {
                        return None;
                    }
                }
            };
        }
    }
}

impl FdsIterData {
    // Safety: cmsg must be properly initalized for cmsg.cmsg_len bytes
    // starting at cmsg. That is, the implict cmsg_data member of cmsg must
    // be properly initalized.
    unsafe fn new(cmsg: &cmsghdr) -> Option<Self> {
        if cmsg.cmsg_level == SOL_SOCKET && cmsg.cmsg_type == SCM_RIGHTS {
            // Safety: follows from pre-condition
            let p_start = CMSG_DATA(cmsg) as *const u8;

            assert!(cmsg.cmsg_len <= (isize::MAX as usize));
            let pcmsg: *const cmsghdr = cmsg;
            // Safety: follows from pre-condition,  from the defintion of a
            // cmsg, and from the assertion above.
            let p_end = (pcmsg.cast::<u8>()).offset(cmsg.cmsg_len as isize);

            let data_size = (p_end as usize) - (p_start as usize);
            // This may round down if the data portion is bigger than an
            // integral number of RawFd's.
            let fds_count: usize = data_size / mem::size_of::<RawFd>();

            let curr = p_start.cast::<RawFd>();
            // Safety: curr points to the first byte of the implict cmsg_data
            // member of the cmsg which is properly initalized by the precondition;
            // curr.offset(fds_count) is <= p_end by the way fds_count is calculated
            // so it is also either in the implict cmsg_data member of cmsg or is
            // one byte past the end; the assertion above guarentees that the offset
            // in bytes implied by fds_count <= isize::MAX.
            let end = curr.offset(fds_count as isize);

            // Invariants:
            //      1. curr is non-null by defintion of CMSG_DATA; end is
            //          non-null by defintion of offset.
            //      2. end is offset from curr by fds_count which is non-negative
            //          so curr <= end.
            //      3. cmsg is properly initalized (by the precondition) and is
            //          of type SCM_RIGHTS which means that its data portion
            //          is an array of RawFd's; the calculations of curr and
            //          end above give the first and one-past-the-last elements
            //          of this array so [curr, end) is the whole array of
            //          RawFd's that make up the the data portion of cmsg.
            //      4. Follows from 3.
            Some(FdsIterData { curr, end })
        } else {
            None
        }
    }
}

impl Iterator for FdsIterData {
    type Item = RawFd;

    fn next(&mut self) -> Option<Self::Item> {
        if self.curr < self.end {
            // Safety: curr < end so follows from invariants 1, 3 and 4.
            let next = unsafe { self.curr.read_unaligned() };

            // Safety: curr < end so curr is valid by invariant 4;
            // curr.offset(1) is either end or and element of
            // [curr, end) by invariant 3 and is thus either one
            // past the end of the RawFd array or in bounds for the
            // RawFd array. The offset in bytes is sizeof::<RawFd>()
            // which is (much) less than isize::MAX.
            //
            // Invariants: invariant 1 is maintained by defintion of offset();
            // invariant 3 (from entry to method) and curr < end means that
            // curr.offset(1) <= end so invariants 2 and 3 are maintained;
            // invariant 3 being maintained imples that invariant 4 is maintained.
            self.curr = unsafe { self.curr.offset(1) };

            Some(next)
        } else {
            None
        }
    }
}

fn call_res<F, R>(mut f: F) -> Result<R, io::Error>
where
    F: FnMut() -> R,
    R: One + Neg<Output = R> + PartialEq,
{
    let res = f();
    if res == -R::one() {
        Err(io::Error::last_os_error())
    } else {
        Ok(res)
    }
}
