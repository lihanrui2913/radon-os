use alloc::boxed::Box;
use alloc::collections::{BTreeMap, VecDeque};
use alloc::sync::Arc;
use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll};
use radon_kernel::Result;
use spin::Mutex;

use crate::port::{Deadline, Port, PortPacket};

use super::waker::{TaskId, TaskWaker};

/// 任务类型
type TaskFuture = Pin<Box<dyn Future<Output = ()> + Send + 'static>>;

/// 任务
struct Task {
    future: TaskFuture,
    /// 关联的 port key（用于异步等待）
    port_key: Option<u64>,
}

/// 执行器
pub struct Executor {
    /// 事件 Port
    port: Port,
    /// 任务表
    tasks: Mutex<BTreeMap<TaskId, Task>>,
    /// 就绪队列
    ready_queue: Arc<Mutex<VecDeque<TaskId>>>,
    /// 下一个任务 ID
    next_task_id: Mutex<u64>,
    /// 下一个 port key
    next_port_key: Mutex<u64>,
    /// key -> task_id 映射
    key_to_task: Mutex<BTreeMap<u64, TaskId>>,
}

impl Executor {
    /// 创建新的执行器
    pub fn new() -> Result<Self> {
        let port = Port::create()?;

        Ok(Self {
            port,
            tasks: Mutex::new(BTreeMap::new()),
            ready_queue: Arc::new(Mutex::new(VecDeque::new())),
            next_task_id: Mutex::new(1),
            next_port_key: Mutex::new(1),
            key_to_task: Mutex::new(BTreeMap::new()),
        })
    }

    /// 获取 Port 引用
    pub fn port(&self) -> &Port {
        &self.port
    }

    /// 分配 port key
    pub fn alloc_key(&self) -> u64 {
        let mut next = self.next_port_key.lock();
        let key = *next;
        *next += 1;
        key
    }

    /// 注册 key 到任务映射
    pub fn register_key(&self, key: u64, task_id: TaskId) {
        self.key_to_task.lock().insert(key, task_id);
    }

    /// 移除 key 映射
    pub fn unregister_key(&self, key: u64) {
        self.key_to_task.lock().remove(&key);
    }

    /// 生成新任务
    pub fn spawn<F>(&self, future: F) -> TaskId
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let id = {
            let mut next = self.next_task_id.lock();
            let id = TaskId(*next);
            *next += 1;
            id
        };

        let task = Task {
            future: Box::pin(future),
            port_key: None,
        };

        self.tasks.lock().insert(id, task);
        self.ready_queue.lock().push_back(id);

        id
    }

    /// 运行直到所有任务完成
    pub fn run(&self) {
        let mut packets = [PortPacket::zeroed(); 32];

        loop {
            // 处理就绪任务
            self.poll_ready_tasks();

            // 检查是否还有任务
            if self.tasks.lock().is_empty() {
                break;
            }

            // 等待事件
            match self.port.wait(&mut packets, Deadline::Infinite) {
                Ok(count) => {
                    for packet in &packets[..count] {
                        // 查找对应的任务并唤醒
                        if let Some(&task_id) = self.key_to_task.lock().get(&packet.key) {
                            self.ready_queue.lock().push_back(task_id);
                        }
                    }
                }
                Err(_) => break,
            }
        }
    }

    /// 运行一轮
    pub fn run_once(&self) -> bool {
        let mut packets = [PortPacket::zeroed(); 32];

        // 处理就绪任务
        self.poll_ready_tasks();

        if self.tasks.lock().is_empty() {
            return false;
        }

        // 非阻塞检查事件
        if let Ok(count) = self.port.try_wait(&mut packets) {
            for packet in &packets[..count] {
                if let Some(&task_id) = self.key_to_task.lock().get(&packet.key) {
                    self.ready_queue.lock().push_back(task_id);
                }
            }
        }

        true
    }

    /// Poll 所有就绪任务
    fn poll_ready_tasks(&self) {
        loop {
            let task_id = match self.ready_queue.lock().pop_front() {
                Some(id) => id,
                None => break,
            };

            // 获取任务
            let mut tasks = self.tasks.lock();
            let task = match tasks.get_mut(&task_id) {
                Some(t) => t,
                None => continue,
            };

            // 创建 waker
            let waker_data = Arc::new(TaskWaker::new(task_id, self.ready_queue.clone()));
            let waker = waker_data.into_waker();
            let mut cx = Context::from_waker(&waker);

            // Poll 任务
            let future = &mut task.future;
            match future.as_mut().poll(&mut cx) {
                Poll::Ready(()) => {
                    // 任务完成，移除
                    tasks.remove(&task_id);
                }
                Poll::Pending => {
                    // 任务挂起，等待唤醒
                }
            }
        }
    }
}

/// 阻塞运行 future
pub fn block_on<F, T>(future: F) -> T
where
    F: Future<Output = T>,
{
    // TODO：不用忙等待
    let mut future = core::pin::pin!(future);

    // 创建一个简单的 waker
    let waker = noop_waker();
    let mut cx = Context::from_waker(&waker);

    loop {
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(val) => return val,
            Poll::Pending => {
                // 让出 CPU
                crate::syscall::yield_now();
            }
        }
    }
}

pub fn spawn<F>(future: F) -> Option<TaskId>
where
    F: Future<Output = ()> + Send + 'static,
{
    super::global_executor().map(|e| e.spawn(future))
}

fn noop_waker() -> Waker {
    use core::task::{RawWaker, RawWakerVTable, Waker};

    const VTABLE: RawWakerVTable = RawWakerVTable::new(
        |_| RawWaker::new(core::ptr::null(), &VTABLE),
        |_| {},
        |_| {},
        |_| {},
    );

    unsafe { Waker::from_raw(RawWaker::new(core::ptr::null(), &VTABLE)) }
}

use core::task::Waker;
