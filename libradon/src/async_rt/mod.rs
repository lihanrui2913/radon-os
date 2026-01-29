mod executor;
mod futures;
mod waker;

pub use executor::{Executor, block_on, spawn};
pub use futures::{ChannelRecvFuture, PortWaitFuture, Select, TimeoutFuture};
use radon_kernel::Result;

use alloc::sync::Arc;
use spin::Mutex;

/// 全局执行器（可选）
static GLOBAL_EXECUTOR: Mutex<Option<Arc<Executor>>> = Mutex::new(None);

/// 初始化全局执行器
pub fn init() -> Result<()> {
    let executor = Executor::new()?;
    *GLOBAL_EXECUTOR.lock() = Some(Arc::new(executor));
    Ok(())
}

/// 获取全局执行器
pub fn global_executor() -> Option<Arc<Executor>> {
    GLOBAL_EXECUTOR.lock().clone()
}
