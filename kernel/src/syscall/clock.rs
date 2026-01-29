use crate::{
    Result,
    arch::{CurrentTimeArch, time::TimeArch},
};

pub fn sys_clock_get() -> Result<usize> {
    Ok(CurrentTimeArch::nano_time() as usize)
}
