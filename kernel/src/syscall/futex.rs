use alloc::collections::btree_map::BTreeMap;
use spin::Mutex;

use crate::{EINVAL, EPERM, Error, Result, object::WaitQueue};

static FUTEXES: Mutex<BTreeMap<usize, WaitQueue>> = Mutex::new(BTreeMap::new());

pub fn sys_futex_wait(ptr: usize, val: usize, _deadline: usize) -> Result<usize> {
    let val_user = unsafe { core::ptr::read_unaligned(ptr as *const u32) };
    if val as u32 != val_user {
        return Err(Error::new(EPERM));
    }
    let mut futexes = FUTEXES.lock();
    if let None = futexes.get(&ptr) {
        futexes.insert(ptr, WaitQueue::new());
    }
    let wait_queue = futexes.get_mut(&ptr).unwrap();
    wait_queue.wait();
    Ok(0)
}

pub fn sys_futex_wake(ptr: usize, count: usize) -> Result<usize> {
    let mut futexes = FUTEXES.lock();
    let wait_queue = futexes.get_mut(&ptr).ok_or(Error::new(EINVAL))?;
    let mut wake_count = 0;
    while wait_queue.has_waiters() && wake_count < count {
        wait_queue.wake_one();
        wake_count += 1;
    }
    if !wait_queue.has_waiters() {
        let _ = futexes.remove(&ptr);
    }
    Ok(wake_count)
}
