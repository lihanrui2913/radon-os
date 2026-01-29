use alloc::sync::Arc;
use core::task::{RawWaker, RawWakerVTable, Waker};
use spin::Mutex;

/// 任务 ID
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TaskId(pub u64);

/// 任务唤醒器
pub struct TaskWaker {
    pub task_id: TaskId,
    pub wake_queue: Arc<Mutex<alloc::collections::VecDeque<TaskId>>>,
}

impl TaskWaker {
    pub fn new(
        task_id: TaskId,
        wake_queue: Arc<Mutex<alloc::collections::VecDeque<TaskId>>>,
    ) -> Self {
        Self {
            task_id,
            wake_queue,
        }
    }

    pub fn into_waker(self: Arc<Self>) -> Waker {
        unsafe { Waker::from_raw(Self::into_raw_waker(self)) }
    }

    fn into_raw_waker(this: Arc<Self>) -> RawWaker {
        RawWaker::new(Arc::into_raw(this) as *const (), &VTABLE)
    }
}

// RawWaker 虚表
static VTABLE: RawWakerVTable = RawWakerVTable::new(
    // clone
    |ptr| {
        let arc = unsafe { Arc::from_raw(ptr as *const TaskWaker) };
        let cloned = arc.clone();
        core::mem::forget(arc);
        TaskWaker::into_raw_waker(cloned)
    },
    // wake
    |ptr| {
        let arc = unsafe { Arc::from_raw(ptr as *const TaskWaker) };
        arc.wake_queue.lock().push_back(arc.task_id);
    },
    // wake_by_ref
    |ptr| {
        let arc = unsafe { Arc::from_raw(ptr as *const TaskWaker) };
        arc.wake_queue.lock().push_back(arc.task_id);
        core::mem::forget(arc);
    },
    // drop
    |ptr| {
        unsafe { Arc::from_raw(ptr as *const TaskWaker) };
    },
);
