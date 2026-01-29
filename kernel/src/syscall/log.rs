use alloc::string::String;

use crate::{EINVAL, Error, Result};

pub fn sys_log(buf: usize, len: usize) -> Result<usize> {
    let buf = unsafe { core::slice::from_raw_parts(buf as *const u8, len) }.to_vec();
    let str = String::from_utf8(buf).map_err(|_| Error::new(EINVAL))?;
    crate::serial_print!("{}", str);
    crate::print!("{}", str);
    Ok(len)
}
