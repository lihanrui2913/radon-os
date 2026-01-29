/// 任务状态
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskState {
    /// 已创建，但未启动
    Created,
    /// 就绪，等待调度
    Ready,
    /// 正在运行
    Running,
    /// 阻塞中（等待事件）
    Blocked,
    /// 已停止（可恢复）
    Stopped,
    /// 已退出
    Exited,
}

impl TaskState {
    /// 是否可以被调度执行
    pub fn is_schedulable(&self) -> bool {
        matches!(self, TaskState::Ready | TaskState::Running)
    }

    /// 是否已终止
    pub fn is_terminated(&self) -> bool {
        matches!(self, TaskState::Exited)
    }

    /// 是否可以启动
    pub fn can_start(&self) -> bool {
        matches!(self, TaskState::Created | TaskState::Stopped)
    }
}

/// 进程状态
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessState {
    /// 已创建，未启动
    Created,
    /// 运行中（至少有一个线程在运行）
    Running,
    /// 已停止（所有线程停止）
    Stopped,
    /// 已退出
    Exited,
}
