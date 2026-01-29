use core::arch::asm;

pub mod nr {
    pub use radon_kernel::nr::*;
}

use radon_kernel::{Error, Result};

pub use crate::arch::*;

#[inline]
pub fn result_from_retval(ret: usize) -> Result<usize> {
    let sret = ret as isize;
    if sret < 0 && sret >= -4096 {
        Err(Error::new(-sret as i32))
    } else {
        Ok(ret)
    }
}

pub fn exit(code: i32) -> ! {
    unsafe {
        crate::arch::syscall1(nr::SYS_EXIT, code as usize);
    }
    loop {
        unsafe { asm!("hlt") };
    }
}

pub fn yield_now() {
    unsafe {
        syscall0(nr::SYS_YIELD);
    }
}

pub fn clock_get() -> Result<u64> {
    let ret = unsafe { syscall0(nr::SYS_CLOCK_GET) };
    result_from_retval(ret).map(|v| v as u64)
}

pub fn nanosleep(ns: u64) -> Result<()> {
    let ret = unsafe { syscall1(nr::SYS_NANOSLEEP, ns as usize) };
    result_from_retval(ret).map(|_| ())
}
