use alloc::sync::Arc;
#[cfg(target_arch = "x86_64")]
use x86_64::{VirtAddr, registers::model_specific::FsBase};

use crate::{
    ENOENT, ESRCH, Error, Result,
    arch::Ptrace,
    drivers::acpi::RSDP_REQUEST,
    task::{TASKS, block_task, get_current_task, unblock_task},
};

pub fn get_rsdp() -> Result<usize> {
    RSDP_REQUEST
        .get_response()
        .ok_or(Error::new(ENOENT))
        .map(|rsdp_response| rsdp_response.address())
}

#[cfg(target_arch = "x86_64")]
pub fn get_fsbase(tid: usize) -> Result<usize> {
    let tasks = TASKS.lock();
    let task = tasks
        .iter()
        .find(|t| t.read().tid() == tid)
        .ok_or(Error::new(ESRCH))?;

    Ok(task.read().arch_context.fsbase)
}

#[cfg(target_arch = "x86_64")]
pub fn set_fsbase(tid: usize, fsbase: usize) -> Result<usize> {
    let tasks = TASKS.lock();
    let task = tasks
        .iter()
        .find(|t| t.read().tid() == tid)
        .ok_or(Error::new(ESRCH))?;

    task.write().arch_context.fsbase = fsbase;

    let current = get_current_task().unwrap();
    if Arc::ptr_eq(task, &current) {
        FsBase::write(VirtAddr::new(fsbase as u64));
    }

    Ok(0)
}

pub fn sys_load_task_registers(tid: usize, reg: *mut Ptrace) -> Result<usize> {
    let tasks = TASKS.lock();
    let task = tasks
        .iter()
        .find(|t| t.read().tid() == tid)
        .ok_or(Error::new(ESRCH))?;
    block_task(task.clone());
    let regs_ptr = task.write().pt_regs();
    unsafe {
        reg.write_unaligned(regs_ptr.read_unaligned());
    }
    unblock_task(task.clone());
    Ok(0)
}

pub fn sys_store_task_registers(tid: usize, reg: *const Ptrace) -> Result<usize> {
    let tasks = TASKS.lock();
    let task = tasks
        .iter()
        .find(|t| t.read().tid() == tid)
        .ok_or(Error::new(ESRCH))?;
    block_task(task.clone());
    let regs_ptr = task.write().pt_regs();
    unsafe {
        regs_ptr.write_unaligned(reg.read_unaligned());
    }
    unblock_task(task.clone());
    Ok(0)
}
