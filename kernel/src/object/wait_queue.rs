use alloc::collections::VecDeque;
use alloc::sync::Arc;
use spin::Mutex;

use crate::task::{WeakArcTask, block, get_current_task, schedule, unblock};

/// 等待队列条目
struct Waiter {
    task: WeakArcTask,
    woken: bool,
}

/// 等待队列
pub struct WaitQueue {
    waiters: Mutex<VecDeque<Waiter>>,
}

impl WaitQueue {
    pub const fn new() -> Self {
        Self {
            waiters: Mutex::new(VecDeque::new()),
        }
    }

    /// 阻塞当前任务直到被唤醒
    pub fn wait(&self) {
        let current = match get_current_task() {
            Some(t) => t,
            None => return,
        };

        // 加入等待队列
        {
            let mut waiters = self.waiters.lock();
            waiters.push_back(Waiter {
                task: Arc::downgrade(&current),
                woken: false,
            });
        }

        // 阻塞并调度
        block(current);
        schedule();
    }

    /// 条件等待
    pub fn wait_until<F>(&self, mut condition: F)
    where
        F: FnMut() -> bool,
    {
        while !condition() {
            self.wait();
        }
    }

    /// 唤醒一个等待者
    pub fn wake_one(&self) -> bool {
        loop {
            let task = {
                let mut waiters = self.waiters.lock();
                match waiters.pop_front() {
                    Some(waiter) => waiter.task.upgrade(),
                    None => return false,
                }
            };

            if let Some(task) = task {
                unblock(task);
                return true;
            }
            // 任务已销毁，继续尝试下一个
        }
    }

    /// 唤醒所有等待者
    pub fn wake_all(&self) -> usize {
        let waiters: VecDeque<_> = {
            let mut guard = self.waiters.lock();
            core::mem::take(&mut *guard)
        };

        let mut count = 0;
        for waiter in waiters {
            if let Some(task) = waiter.task.upgrade() {
                unblock(task);
                count += 1;
            }
        }
        count
    }

    /// 是否有等待者
    pub fn has_waiters(&self) -> bool {
        !self.waiters.lock().is_empty()
    }
}
