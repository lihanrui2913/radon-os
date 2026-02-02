use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use alloc::{
    collections::vec_deque::VecDeque,
    string::{String, ToString},
    sync::{Arc, Weak},
};
use rmm::{Arch, FrameAllocator, FrameCount, PhysicalAddress, VirtualAddress};
use spin::{Mutex, RwLock};

use crate::{
    arch::{ArchContext, CurrentRmmArch, Ptrace, get_archid, irq::IrqRegsArch, switch_to},
    consts::STACK_SIZE,
    init::memory::{FRAME_ALLOCATOR, PAGE_SIZE},
    initial_kernel_thread,
    object::process::{ArcProcess, WeakArcProcess},
    smp::{CPU_COUNT, get_archid_by_cpuid},
    task::sched::{ArcScheduler, SCHEDULERS},
};

pub mod sched;
pub mod state;

pub use state::{ProcessState, TaskState};

pub type ArcTask = Arc<RwLock<Task>>;
pub type WeakArcTask = Weak<RwLock<Task>>;

/// 全局任务 ID 计数器
pub static NEXT_TID: AtomicUsize = AtomicUsize::new(1);

/// 线程（任务）
pub struct Task {
    /// 任务 ID（线程 ID）
    tid: usize,
    /// 任务名称
    name: String,
    /// 所属进程
    process: Option<WeakArcProcess>,
    /// 任务状态
    state: TaskState,
    /// 分配的 CPU ID
    cpu_id: usize,
    /// 退出码
    exit_code: Option<i32>,

    /// 内核栈顶
    kernel_stack_top: VirtualAddress,
    /// Syscall 栈顶
    pub syscall_stack_top: VirtualAddress,
    /// 用户态 syscall 栈
    pub user_syscall_stack: VirtualAddress,

    /// 架构相关上下文
    pub arch_context: ArchContext,

    /// 是否正在运行
    pub running: bool,
}

pub const IDLE_PRIORITY: usize = 20;
pub const NORMAL_PRIORITY: usize = 0;

fn alloc_cpuid() -> usize {
    static NEXT_CPUID: AtomicUsize = AtomicUsize::new(0);
    let cpu_count = CPU_COUNT.load(Ordering::SeqCst);
    let next = NEXT_CPUID.fetch_add(1, Ordering::SeqCst);
    next % cpu_count
}

impl Task {
    /// 创建 idle 任务
    pub fn new_idle(cpu_id: usize) -> ArcTask {
        Self::new_inner(0, cpu_id, "idle".to_string(), None, true)
    }

    /// 创建内核任务
    pub fn new_kernel(name: String) -> ArcTask {
        let tid = NEXT_TID.fetch_add(1, Ordering::SeqCst);
        let cpu_id = alloc_cpuid();
        Self::new_inner(tid, cpu_id, name, None, false)
    }

    /// 创建用户任务（属于某个进程）
    pub fn new_user(name: String, process: ArcProcess) -> ArcTask {
        let tid = NEXT_TID.fetch_add(1, Ordering::SeqCst);
        let cpu_id = alloc_cpuid();
        Self::new_inner(tid, cpu_id, name, Some(process), false)
    }

    fn new_inner(
        tid: usize,
        cpu_id: usize,
        name: String,
        process: Option<ArcProcess>,
        is_idle: bool,
    ) -> ArcTask {
        let stack_frame_count = FrameCount::new(STACK_SIZE / PAGE_SIZE);

        let kernel_stack_phys = unsafe { FRAME_ALLOCATOR.lock().allocate(stack_frame_count) }
            .expect("No memory to allocate kernel stack");
        let kernel_stack_virt = unsafe { CurrentRmmArch::phys_to_virt(kernel_stack_phys) };

        let syscall_stack_phys = unsafe { FRAME_ALLOCATOR.lock().allocate(stack_frame_count) }
            .expect("No memory to allocate syscall stack");
        let syscall_stack_virt = unsafe { CurrentRmmArch::phys_to_virt(syscall_stack_phys) };

        let task = Task {
            tid,
            name,
            process: process.map(|p| Arc::downgrade(&p)),
            state: if is_idle {
                TaskState::Ready
            } else {
                TaskState::Created
            },
            cpu_id,
            exit_code: None,
            kernel_stack_top: kernel_stack_virt.add(STACK_SIZE),
            syscall_stack_top: syscall_stack_virt.add(STACK_SIZE),
            user_syscall_stack: VirtualAddress::new(0),
            arch_context: ArchContext::default(),
            running: false,
        };

        Arc::new(RwLock::new(task))
    }

    pub fn tid(&self) -> usize {
        self.tid
    }

    pub fn get_name(&self) -> String {
        self.name.clone()
    }

    pub fn state(&self) -> TaskState {
        self.state
    }

    pub fn set_state(&mut self, state: TaskState) {
        self.state = state;
    }

    pub fn process(&self) -> Option<ArcProcess> {
        self.process.as_ref().and_then(|p| p.upgrade())
    }

    pub fn get_cpu_id(&self) -> usize {
        self.cpu_id
    }

    pub fn get_kernel_stack_top(&self) -> VirtualAddress {
        self.kernel_stack_top
    }

    pub fn exit_code(&self) -> Option<i32> {
        self.exit_code
    }

    pub fn set_exit_code(&mut self, code: i32) {
        self.exit_code = Some(code);
    }

    pub fn pt_regs(&self) -> *mut Ptrace {
        unsafe { (self.kernel_stack_top.data() as *mut Ptrace).sub(1) }
    }

    pub fn set_kernel_context_info(&mut self, entry: usize, stack_top: VirtualAddress) {
        let regs = unsafe { self.pt_regs().as_mut_unchecked() };
        regs.set_user_space(false);
        regs.set_args((0, 0, entry as u64, 0, 0, 0));
        self.arch_context.ip = crate::arch::kernel_thread_entry as *const () as usize;
        self.arch_context.sp = stack_top.data();
    }

    pub fn set_user_context_info(
        &mut self,
        entry: usize,
        stack_top: VirtualAddress,
        args: Option<(usize, usize, usize, usize, usize, usize)>,
    ) {
        self.arch_context.ip = crate::arch::return_from_interrupt as *const () as usize;
        self.arch_context.sp = self.pt_regs() as usize;
        let regs = unsafe { self.pt_regs().as_mut_unchecked() };
        regs.set_user_space(true);
        regs.set_ip(entry as u64);
        regs.set_sp(stack_top.data() as u64);
        if let Some(args) = args {
            let args = (
                args.0 as u64,
                args.1 as u64,
                args.2 as u64,
                args.3 as u64,
                args.4 as u64,
                args.5 as u64,
            );
            regs.set_args(args);
        }
    }
}

impl Drop for Task {
    fn drop(&mut self) {
        let mut frame_allocator = FRAME_ALLOCATOR.lock();
        let stack_frame_count = FrameCount::new(STACK_SIZE / PAGE_SIZE);

        let kernel_stack_phys = PhysicalAddress::new(
            self.kernel_stack_top
                .sub(CurrentRmmArch::PHYS_OFFSET)
                .data(),
        )
        .sub(STACK_SIZE);
        unsafe { frame_allocator.free(kernel_stack_phys, stack_frame_count) };

        let syscall_stack_phys = PhysicalAddress::new(
            self.syscall_stack_top
                .sub(CurrentRmmArch::PHYS_OFFSET)
                .data(),
        )
        .sub(STACK_SIZE);
        unsafe { frame_allocator.free(syscall_stack_phys, stack_frame_count) };
    }
}

pub static TASKS: Mutex<VecDeque<ArcTask>> = Mutex::new(VecDeque::new());

/// 添加任务到全局列表（不启动）
pub fn register_task(task: ArcTask) {
    TASKS.lock().push_back(task);
}

/// 从全局列表移除任务
pub fn unregister_task(task: &ArcTask) {
    let mut tasks = TASKS.lock();
    if let Some(pos) = tasks.iter().position(|t| Arc::ptr_eq(t, task)) {
        tasks.remove(pos);
    }
}

/// 启动任务（加入调度器）
pub fn start_task(task: ArcTask) {
    {
        let mut t = task.write();
        if !t.state.can_start() {
            return;
        }
        t.set_state(TaskState::Ready);
    }

    let cpu_id = task.read().get_cpu_id();
    let archid = get_archid_by_cpuid(cpu_id);
    get_scheduler_by_archid(archid).write().add_task(task);
}

/// 阻塞任务
pub fn block_task(task: ArcTask) {
    {
        let mut t = task.write();
        t.set_state(TaskState::Blocked);
    }

    let cpu_id = task.read().get_cpu_id();
    let scheduler = get_scheduler_by_cpuid(cpu_id);
    scheduler.write().block_task(task.clone());

    let need_delay = cpu_id != get_current_task().unwrap().read().cpu_id;
    while need_delay && task.read().running {
        schedule();
    }
}

/// 解除阻塞
pub fn unblock_task(task: ArcTask) {
    {
        let mut t = task.write();
        if t.state != TaskState::Blocked {
            return;
        }
        t.set_state(TaskState::Ready);
    }

    let cpu_id = task.read().get_cpu_id();
    let scheduler = get_scheduler_by_cpuid(cpu_id);
    scheduler.write().unblock_task(task);
}

/// 停止任务
pub fn stop_task(task: ArcTask) {
    {
        let mut t = task.write();
        t.set_state(TaskState::Stopped);
    }

    let cpu_id = task.read().get_cpu_id();
    let scheduler = get_scheduler_by_cpuid(cpu_id);
    scheduler.write().stop_task(task);
}

/// 退出任务
pub fn exit_task(task: ArcTask, exit_code: i32) {
    {
        let mut t = task.write();
        t.set_state(TaskState::Exited);
        t.set_exit_code(exit_code);
    }

    let cpu_id = task.read().get_cpu_id();
    let scheduler = get_scheduler_by_cpuid(cpu_id);
    scheduler.write().remove_task(task.clone());

    // 通知所属进程
    if let Some(process) = task.clone().read().process() {
        process.write().on_thread_exit(task);
    }
}

pub fn get_scheduler_by_archid(archid: usize) -> ArcScheduler {
    SCHEDULERS.lock().get(&archid).unwrap().clone()
}

pub fn get_scheduler_by_cpuid(cpuid: usize) -> ArcScheduler {
    SCHEDULERS
        .lock()
        .get(&get_archid_by_cpuid(cpuid))
        .unwrap()
        .clone()
}

pub fn get_scheduler() -> ArcScheduler {
    get_scheduler_by_archid(get_archid())
}

pub fn get_current_task() -> Option<ArcTask> {
    get_scheduler().read().get_current_task()
}

/// 创建并启动内核任务
pub fn create_kernel_task(name: String, entry: usize) -> Option<ArcTask> {
    let task = Task::new_kernel(name);
    {
        let mut task_guard = task.write();
        let stack_top = VirtualAddress::new(task_guard.pt_regs() as usize);
        task_guard.set_kernel_context_info(entry, stack_top);
    }
    register_task(task.clone());
    start_task(task.clone());
    Some(task)
}

/// 退出当前任务
pub fn exit_current(exit_code: i32) -> ! {
    let current = get_current_task().unwrap();
    exit_task(current, exit_code);
    loop {
        schedule();
    }
}

pub static TASK_INITIALIZED: AtomicBool = AtomicBool::new(false);

/// 初始化调度系统
pub fn init() -> Option<ArcTask> {
    for cpu_id in 0..CPU_COUNT.load(Ordering::SeqCst) {
        let idle_task = Task::new_idle(cpu_id);
        TASKS.lock().push_back(idle_task.clone());
        let scheduler = get_scheduler_by_cpuid(cpu_id);
        scheduler.write().set_idle_task(idle_task);
    }

    let task = create_kernel_task(
        "init".to_string(),
        initial_kernel_thread as *const () as usize,
    );

    TASK_INITIALIZED.store(true, Ordering::SeqCst);

    task
}

/// 调度
pub fn schedule() {
    let current_scheduler = get_scheduler();
    let prev = current_scheduler
        .read()
        .get_current_task()
        .expect("Scheduler not initialized");
    prev.write().running = false;
    let next = current_scheduler.write().schedule();
    next.write().running = true;
    drop(current_scheduler);
    switch_to(prev, next);
}

pub fn block(task: ArcTask) {
    block_task(task);
}

pub fn unblock(task: ArcTask) {
    unblock_task(task);
}
