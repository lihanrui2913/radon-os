use alloc::{
    string::String,
    sync::{Arc, Weak},
    vec::Vec,
};
use core::any::Any;
use core::sync::atomic::{AtomicI32, AtomicUsize, Ordering};
use rmm::{PhysicalAddress, VirtualAddress};
use spin::{Mutex, RwLock};

use crate::{
    loader::program::LOADED_PROGRAMS,
    task::{register_task, start_task, stop_task},
};
use crate::{
    object::vmar::Vmar,
    task::{ArcTask, ProcessState, Task, WeakArcTask},
};

use super::{
    Handle, HandleTable, KernelObject, ObjectType, Rights, SignalObserver, SignalState, Signals,
    channel::Channel,
};

/// 用户地址空间配置
pub mod layout {
    /// 用户空间起始地址
    pub const USER_SPACE_START: usize = 0x0000_0000_0000_1000;
    /// 用户空间结束地址
    pub const USER_SPACE_END: usize = 0x0000_7FFF_FFFF_0000;
    /// 默认栈大小 (8 MB)
    pub const DEFAULT_STACK_SIZE: usize = 8 * 1024 * 1024;
    /// 栈顶地址
    pub const STACK_TOP: usize = 0x0000_7FFF_FFFF_0000;
    /// 堆起始地址（动态确定）
    pub const ALLOC_START: usize = 0x0000_1000_0000_0000;
}

pub type ArcProcess = Arc<RwLock<Process>>;
pub type WeakArcProcess = Weak<RwLock<Process>>;

/// 全局进程 ID 计数器
static NEXT_PID: AtomicUsize = AtomicUsize::new(1);

/// 进程对象
pub struct Process {
    /// 进程 ID
    pid: usize,
    /// 进程名称
    name: String,
    /// 进程状态
    state: ProcessState,
    /// 退出码
    exit_code: AtomicI32,

    /// 父进程
    parent: Option<WeakArcProcess>,
    /// 子进程列表
    children: Vec<WeakArcProcess>,

    /// 主线程
    main_thread: Option<WeakArcTask>,
    /// 所有线程
    threads: Vec<WeakArcTask>,

    /// 句柄表
    handles: HandleTable,

    /// 初始句柄（进程启动时可用）
    init_handles: Vec<Handle>,
    /// Bootstrap channel
    bootstrap_channel: Option<Handle>,

    /// 信号状态
    signal_state: SignalState,

    /// 自身弱引用
    self_ref: Option<WeakArcProcess>,

    /// 根 VMAR（进程的地址空间）
    root_vmar: Option<Arc<Vmar>>,
}

impl Process {
    /// 创建新进程
    pub fn new(name: String, parent: Option<ArcProcess>) -> ArcProcess {
        let pid = NEXT_PID.fetch_add(1, Ordering::SeqCst);

        // 继承父进程的页表，或使用内核页表
        let page_table_addr = if let Some(ref parent) = parent {
            parent
                .read()
                .root_vmar()
                .unwrap()
                .page_table_addr()
                .unwrap()
        } else {
            #[cfg(any(target_arch = "x86_64", target_arch = "riscv64"))]
            {
                use crate::init::memory::KERNEL_PAGE_TABLE_PHYS;
                PhysicalAddress::new(KERNEL_PAGE_TABLE_PHYS.load(Ordering::SeqCst))
            }
            #[cfg(any(target_arch = "aarch64", target_arch = "loongarch64"))]
            {
                PhysicalAddress::new(0)
            }
        };

        let user_base = VirtualAddress::new(layout::USER_SPACE_START);
        let user_size = layout::USER_SPACE_END - layout::USER_SPACE_START;
        let root_vmar = Vmar::create_root(user_base, user_size, user_base.data(), page_table_addr);

        let process = Arc::new(RwLock::new(Process {
            pid,
            name,
            state: ProcessState::Created,
            exit_code: AtomicI32::new(0),
            parent: parent.map(|p| Arc::downgrade(&p)),
            children: Vec::new(),
            main_thread: None,
            threads: Vec::new(),
            handles: HandleTable::new(),
            init_handles: Vec::new(),
            bootstrap_channel: None,
            signal_state: SignalState::new(),
            self_ref: None,
            root_vmar: Some(root_vmar),
        }));

        // 设置自身引用
        process.write().self_ref = Some(Arc::downgrade(&process));

        // 添加到父进程的子进程列表
        if let Some(parent) = process.read().parent.as_ref().and_then(|p| p.upgrade()) {
            parent.write().children.push(Arc::downgrade(&process));
        }

        process
    }

    /// 创建进程并自动创建 bootstrap channel
    pub fn new_with_bootstrap(
        name: String,
        parent: Option<ArcProcess>,
    ) -> (ArcProcess, Option<Arc<Channel>>) {
        let process = Self::new(name, parent.clone());

        // 创建 bootstrap channel
        let (parent_end, child_end) = Channel::create_pair();

        // 将子进程端的 channel 添加到子进程的句柄表
        let child_handle = process.write().handles.insert(
            child_end.clone() as Arc<dyn KernelObject>,
            Rights::BASIC | Rights::TRANSFER,
        );
        process.write().bootstrap_channel = Some(child_handle);

        (process, Some(parent_end))
    }

    pub fn pid(&self) -> usize {
        self.pid
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn state(&self) -> ProcessState {
        self.state
    }

    pub fn exit_code(&self) -> i32 {
        self.exit_code.load(Ordering::SeqCst)
    }

    pub fn handles(&self) -> &HandleTable {
        &self.handles
    }

    pub fn handles_mut(&mut self) -> &mut HandleTable {
        &mut self.handles
    }

    pub fn bootstrap_handle(&self) -> Option<Handle> {
        self.bootstrap_channel
    }

    pub fn init_handles(&self) -> &[Handle] {
        &self.init_handles
    }

    pub fn main_thread(&self) -> Option<ArcTask> {
        self.main_thread.as_ref().and_then(|t| t.upgrade())
    }

    pub fn parent(&self) -> Option<ArcProcess> {
        self.parent.as_ref().and_then(|p| p.upgrade())
    }

    fn self_arc(&self) -> Option<ArcProcess> {
        self.self_ref.as_ref().and_then(|r| r.upgrade())
    }

    pub fn set_root_vmar(&mut self, root_vmar: Arc<Vmar>) {
        self.root_vmar = Some(root_vmar);
    }

    /// 设置 BRK
    pub fn set_brk(&mut self, _brk: VirtualAddress) {}

    /// 创建主线程（进程的第一个线程）
    pub fn create_main_thread(&mut self, entry: usize, stack_top: usize) -> Option<ArcTask> {
        if self.main_thread.is_some() {
            return None;
        }

        let process_arc = self.self_arc()?;
        let task = Task::new_user(format!("{}/main", self.name), process_arc);

        {
            let mut t = task.write();
            t.set_user_context_info(entry, rmm::VirtualAddress::new(stack_top), None);
        }

        register_task(task.clone());

        self.main_thread = Some(Arc::downgrade(&task));
        self.threads.push(Arc::downgrade(&task));

        Some(task)
    }

    /// 创建额外的线程
    pub fn create_thread(
        &mut self,
        name: String,
        entry: usize,
        stack_top: usize,
    ) -> Option<ArcTask> {
        let process_arc = self.self_arc()?;
        let task = Task::new_user(name, process_arc);

        {
            let mut t = task.write();
            t.set_user_context_info(entry, rmm::VirtualAddress::new(stack_top), None);
        }

        register_task(task.clone());
        self.threads.push(Arc::downgrade(&task));

        Some(task)
    }

    /// 启动进程（启动所有线程）
    pub fn start(&mut self) {
        if self.state != ProcessState::Created && self.state != ProcessState::Stopped {
            return;
        }

        self.state = ProcessState::Running;

        // 启动所有线程
        for thread_weak in &self.threads {
            if let Some(thread) = thread_weak.upgrade() {
                let state = thread.read().state();
                if state.can_start() {
                    start_task(thread);
                }
            }
        }

        // 设置信号
        self.signal_state.clear(Signals::TERMINATED);
    }

    /// 停止进程
    pub fn stop(&mut self) {
        if self.state != ProcessState::Running {
            return;
        }

        self.state = ProcessState::Stopped;

        // 停止所有线程
        for thread_weak in &self.threads {
            if let Some(thread) = thread_weak.upgrade() {
                stop_task(thread);
            }
        }
    }

    /// 线程退出回调
    pub fn on_thread_exit(&mut self, task: ArcTask) {
        // 从线程列表移除
        self.threads.retain(|t| {
            t.upgrade()
                .map(|t| !Arc::ptr_eq(&t, &task))
                .unwrap_or(false)
        });

        // 如果是主线程退出，进程也退出
        let is_main = self
            .main_thread
            .as_ref()
            .and_then(|m| m.upgrade())
            .map(|m| Arc::ptr_eq(&m, &task))
            .unwrap_or(false);

        if is_main || self.threads.is_empty() {
            let exit_code = task.read().exit_code().unwrap_or(0);
            self.exit(exit_code);
        }
    }

    /// 进程退出
    pub fn exit(&mut self, exit_code: i32) {
        if self.state == ProcessState::Exited {
            return;
        }

        self.state = ProcessState::Exited;
        self.exit_code.store(exit_code, Ordering::SeqCst);

        // 终止所有剩余线程
        for thread_weak in self.threads.drain(..) {
            if let Some(thread) = thread_weak.upgrade() {
                crate::task::exit_task(thread, exit_code);
            }
        }

        // 设置 TERMINATED 信号
        self.signal_state.set(Signals::TERMINATED);

        // 清理句柄表
        // self.handles.clear();
    }

    /// 添加初始句柄
    pub fn add_init_handle(&mut self, object: Arc<dyn KernelObject>, rights: Rights) -> Handle {
        let handle = self.handles.insert(object, rights);
        self.init_handles.push(handle);
        handle
    }

    /// 从另一个进程复制句柄
    pub fn copy_handle_from(
        &mut self,
        source: &Process,
        handle: Handle,
        rights: Rights,
    ) -> Option<Handle> {
        let obj = source.handles.get(handle, Rights::TRANSFER)?;
        Some(self.handles.insert(obj, rights))
    }

    /// 获取根 VMAR
    pub fn root_vmar(&self) -> Option<Arc<Vmar>> {
        self.root_vmar.clone()
    }
}

#[allow(unused_variables)]
impl KernelObject for Process {
    fn object_type(&self) -> ObjectType {
        ObjectType::Process
    }

    fn signals(&self) -> Signals {
        self.signal_state.get()
    }

    fn signal_set(&self, signals: Signals) {
        // 需要内部可变性
    }

    fn signal_clear(&self, signals: Signals) {
        // 需要内部可变性
    }

    fn add_signal_observer(&self, observer: SignalObserver) {
        // 需要内部可变性
    }

    fn remove_signal_observer(&self, key: u64) {
        // 需要内部可变性
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// 为 RwLock<Process> 实现 KernelObject
impl KernelObject for RwLock<Process> {
    fn object_type(&self) -> ObjectType {
        ObjectType::Process
    }

    fn signals(&self) -> Signals {
        self.read().signal_state.get()
    }

    fn signal_set(&self, signals: Signals) {
        self.write().signal_state.set(signals);
    }

    fn signal_clear(&self, signals: Signals) {
        self.write().signal_state.clear(signals);
    }

    fn add_signal_observer(&self, observer: SignalObserver) {
        self.write().signal_state.add_observer(observer);
    }

    fn remove_signal_observer(&self, key: u64) {
        self.write().signal_state.remove_observer(key);
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

use alloc::collections::BTreeMap;

static PROCESSES: Mutex<BTreeMap<usize, ArcProcess>> = Mutex::new(BTreeMap::new());

/// 注册进程
pub fn register_process(process: ArcProcess) {
    let pid = process.read().pid();
    PROCESSES.lock().insert(pid, process);
}

/// 取消注册进程
pub fn unregister_process(pid: usize) {
    PROCESSES.lock().remove(&pid);
    LOADED_PROGRAMS.lock().remove(&pid);
}

/// 通过 PID 获取进程
pub fn get_process(pid: usize) -> Option<ArcProcess> {
    PROCESSES.lock().get(&pid).cloned()
}

/// 获取当前进程
pub fn current_process() -> Option<ArcProcess> {
    crate::task::get_current_task().and_then(|t| t.read().process())
}
