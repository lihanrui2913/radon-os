use crate::{
    Result,
    arch::{CurrentTimeArch, time::TimeArch},
};

pub fn sys_clock_get() -> Result<usize> {
    Ok(CurrentTimeArch::nano_time() as usize)
}

pub fn sys_nanosleep(ns: usize) -> Result<usize> {
    let start_ns = CurrentTimeArch::nano_time();
    while CurrentTimeArch::nano_time() - start_ns < ns as u64 {
        crate::task::schedule();
    }
    Ok(0)
}
