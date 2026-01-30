use alloc::string::String;
use spin::Mutex;

use crate::{EINVAL, Error, Result};

pub struct UserLogger;

impl UserLogger {
    fn log(&self, s: &str) -> core::fmt::Result {
        crate::serial_print!("{}", s);
        crate::print!("{}", s);
        Ok(())
    }
}

pub static LOCKED_USER_LOGGER: Mutex<UserLogger> = Mutex::new(UserLogger);

pub fn sys_log(buf: usize, len: usize) -> Result<usize> {
    let buf = unsafe { core::slice::from_raw_parts(buf as *const u8, len) }.to_vec();
    let string = String::from_utf8(buf).map_err(|_| Error::new(EINVAL))?;
    LOCKED_USER_LOGGER.lock().log(&string).unwrap();
    Ok(len)
}
