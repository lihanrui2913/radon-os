use alloc::string::String;

use crate::{EINVAL, Error, Result};

pub const LOG_LEVEL_DEBUG: usize = 1;
pub const LOG_LEVEL_INFO: usize = 2;
pub const LOG_LEVEL_WARN: usize = 3;
pub const LOG_LEVEL_ERROR: usize = 4;

pub fn sys_log(level: usize, buf: usize, len: usize) -> Result<usize> {
    let buf = unsafe { core::slice::from_raw_parts(buf as *const u8, len) }.to_vec();
    let str = String::from_utf8(buf).map_err(|_| Error::new(EINVAL))?;
    match level {
        LOG_LEVEL_DEBUG => debug!("{}", str),
        LOG_LEVEL_INFO => info!("{}", str),
        LOG_LEVEL_WARN => warn!("{}", str),
        LOG_LEVEL_ERROR => error!("{}", str),
        _ => {}
    }
    Ok(len)
}
