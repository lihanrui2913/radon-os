use alloc::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Weak},
};
use spin::{Mutex, RwLock};

use crate::task::{ArcTask, TaskState};

pub type ArcScheduler = Arc<RwLock<Scheduler>>;
pub type WeakArcScheduler = Weak<RwLock<Scheduler>>;

/// 调度器
pub struct Scheduler {
    /// Idle 任务
    idle: Option<ArcTask>,
    /// 当前运行的任务
    current: Option<ArcTask>,
    /// 就绪队列
    ready_queue: VecDeque<ArcTask>,
    /// 阻塞列表
    blocked_list: VecDeque<ArcTask>,
    /// 停止列表（已创建但未启动）
    stopped_list: VecDeque<ArcTask>,
}

impl Scheduler {
    pub fn new() -> ArcScheduler {
        Arc::new(RwLock::new(Scheduler {
            idle: None,
            current: None,
            ready_queue: VecDeque::new(),
            blocked_list: VecDeque::new(),
            stopped_list: VecDeque::new(),
        }))
    }

    /// 设置 idle 任务
    pub fn set_idle_task(&mut self, task: ArcTask) {
        self.idle = Some(task.clone());
        self.current = Some(task);
    }

    /// 获取当前任务
    pub fn get_current_task(&self) -> Option<ArcTask> {
        self.current.clone().or_else(|| self.idle.clone())
    }

    /// 设置当前任务
    pub fn set_current_task(&mut self, task: ArcTask) {
        if self.idle.is_none() {
            self.idle = Some(task.clone());
        }
        self.current = Some(task);
    }

    /// 添加任务到就绪队列
    pub fn add_task(&mut self, task: ArcTask) {
        // 确保任务不在其他队列中
        self.remove_from_all_queues(&task);

        // 设置状态并加入就绪队列
        task.write().set_state(TaskState::Ready);
        self.ready_queue.push_back(task);
    }

    /// 从就绪队列移除任务
    pub fn remove_task(&mut self, task: ArcTask) {
        self.remove_from_all_queues(&task);

        // 如果是当前任务，标记为待移除
        if let Some(ref current) = self.current {
            if Arc::ptr_eq(current, &task) {
                // 只是标记任务状态，不在schedule中放回
                task.write().set_state(TaskState::Stopped);
            }
        }
    }

    /// 阻塞当前任务
    pub fn block_current_task(&mut self) {
        if let Some(current) = &self.current {
            // 将任务状态设置为阻塞，但不从current中移除
            current.write().set_state(TaskState::Blocked);
            // 将任务添加到阻塞列表
            self.blocked_list.push_back(current.clone());
        }
    }

    /// 阻塞指定任务
    pub fn block_task(&mut self, task: ArcTask) {
        // 如果阻塞的是当前任务，使用block_current_task
        if let Some(current) = &self.current {
            if Arc::ptr_eq(current, &task) {
                self.block_current_task();
                return;
            }
        }

        // 对于非当前任务，直接从所有队列移除并加入阻塞列表
        self.remove_from_all_queues(&task);
        task.write().set_state(TaskState::Blocked);
        self.blocked_list.push_back(task);
    }

    /// 解除阻塞
    pub fn unblock_task(&mut self, task: ArcTask) {
        // 从阻塞列表移除
        if let Some(pos) = self.blocked_list.iter().position(|t| Arc::ptr_eq(t, &task)) {
            self.blocked_list.remove(pos);
        }

        // 加入就绪队列
        task.write().set_state(TaskState::Ready);
        self.ready_queue.push_back(task);
    }

    /// 停止任务
    pub fn stop_task(&mut self, task: ArcTask) {
        // 如果停止的是当前任务
        if let Some(current) = &self.current {
            if Arc::ptr_eq(current, &task) {
                // 标记状态，不在schedule中放回
                task.write().set_state(TaskState::Stopped);
                return;
            }
        }

        // 对于非当前任务，直接从所有队列移除
        self.remove_from_all_queues(&task);
        task.write().set_state(TaskState::Stopped);
        self.stopped_list.push_back(task);
    }

    /// 恢复停止的任务
    pub fn resume_task(&mut self, task: ArcTask) {
        // 从停止列表移除
        if let Some(pos) = self.stopped_list.iter().position(|t| Arc::ptr_eq(t, &task)) {
            self.stopped_list.remove(pos);
        }

        // 加入就绪队列
        task.write().set_state(TaskState::Ready);
        self.ready_queue.push_back(task);
    }

    /// 调度：选择下一个要运行的任务
    pub fn schedule(&mut self) -> ArcTask {
        // 处理当前任务
        if let Some(current) = self.current.take() {
            let state = current.read().state();

            match state {
                // 如果是可调度状态（Ready/Running），放回就绪队列
                TaskState::Ready | TaskState::Running => {
                    current.write().set_state(TaskState::Ready);
                    self.ready_queue.push_back(current);
                }
                // 阻塞状态：移动到阻塞列表（如果不在列表中）
                TaskState::Blocked => {
                    // 确保在阻塞列表中
                    if !self.blocked_list.iter().any(|t| Arc::ptr_eq(t, &current)) {
                        self.blocked_list.push_back(current.clone());
                    }
                    // 不放回就绪队列
                }
                // 其他状态（Terminated/Stopped）：不放回任何队列
                _ => {}
            }
        }

        // 从就绪队列取出下一个任务
        if let Some(next) = self.ready_queue.pop_front() {
            next.write().set_state(TaskState::Running);
            self.current = Some(next.clone());
            next
        } else {
            // 没有就绪任务，运行 idle
            let idle = self.idle.clone().expect("No idle task");
            idle.write().set_state(TaskState::Running);
            self.current = Some(idle.clone());
            idle
        }
    }

    /// 从所有队列中移除任务（不包括current）
    fn remove_from_all_queues(&mut self, task: &ArcTask) {
        // 从就绪队列移除
        if let Some(pos) = self.ready_queue.iter().position(|t| Arc::ptr_eq(t, task)) {
            self.ready_queue.remove(pos);
        }

        // 从阻塞列表移除
        if let Some(pos) = self.blocked_list.iter().position(|t| Arc::ptr_eq(t, task)) {
            self.blocked_list.remove(pos);
        }

        // 从停止列表移除
        if let Some(pos) = self.stopped_list.iter().position(|t| Arc::ptr_eq(t, task)) {
            self.stopped_list.remove(pos);
        }
    }

    /// 获取就绪任务数量
    pub fn ready_count(&self) -> usize {
        self.ready_queue.len()
    }

    /// 获取阻塞任务数量
    pub fn blocked_count(&self) -> usize {
        self.blocked_list.len()
    }

    /// 获取停止任务数量
    pub fn stopped_count(&self) -> usize {
        self.stopped_list.len()
    }
}

pub static SCHEDULERS: Mutex<BTreeMap<usize, ArcScheduler>> = Mutex::new(BTreeMap::new());
