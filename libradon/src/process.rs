use alloc::vec::Vec;

use crate::channel::Channel;
use crate::handle::{Handle, OwnedHandle, Rights};
use crate::syscall::{self, nr, result_from_retval};
use radon_kernel::{EINVAL, Error, Result};

/// 进程创建选项
#[repr(C)]
struct ProcessCreateOptions {
    name_ptr: usize,
    name_len: usize,
    create_bootstrap: bool,
}

/// 进程创建结果
#[repr(C)]
struct ProcessCreateResult {
    process_handle: u32,
    bootstrap_handle: u32,
}

/// 线程创建选项
#[repr(C)]
struct ThreadCreateOptions {
    process_handle: u32,
    name_ptr: usize,
    name_len: usize,
    entry: usize,
    stack_top: usize,
    arg: usize,
}

/// 进程句柄
pub struct Process {
    handle: OwnedHandle,
    bootstrap: Option<Channel>,
}

impl Process {
    /// 创建新进程（不启动）
    pub fn create(name: &str) -> Result<ProcessBuilder> {
        Ok(ProcessBuilder::new(name))
    }

    /// 从句柄创建
    pub fn from_handle(handle: OwnedHandle) -> Self {
        Self {
            handle,
            bootstrap: None,
        }
    }

    /// 获取句柄
    pub fn handle(&self) -> Handle {
        self.handle.handle()
    }

    /// 获取 bootstrap channel
    pub fn bootstrap(&self) -> Option<&Channel> {
        self.bootstrap.as_ref()
    }

    /// 取出 bootstrap channel
    pub fn take_bootstrap(&mut self) -> Option<Channel> {
        self.bootstrap.take()
    }

    pub fn get_vmar_handle(&self) -> Result<Handle> {
        let ret = unsafe {
            syscall::syscall1(
                nr::SYS_PROCESS_GET_VMAR_HANDLE,
                self.handle().raw() as usize,
            )
        };
        result_from_retval(ret)?;
        Ok(Handle::from_raw(ret as u32))
    }

    /// 启动进程
    pub fn start(&self) -> Result<()> {
        let ret = unsafe { syscall::syscall1(nr::SYS_PROCESS_START, self.handle.raw() as usize) };
        result_from_retval(ret).map(|_| ())
    }

    /// 在进程中创建线程（不启动）
    pub fn create_thread(
        &self,
        name: &str,
        entry: usize,
        stack_top: usize,
        arg: usize,
    ) -> Result<u32> {
        let options = ThreadCreateOptions {
            process_handle: self.handle.raw(),
            name_ptr: name.as_ptr() as usize,
            name_len: name.len(),
            entry,
            stack_top,
            arg,
        };

        let mut thread_id: u32 = 0;

        let ret = unsafe {
            syscall::syscall2(
                nr::SYS_THREAD_CREATE,
                &options as *const _ as usize,
                &mut thread_id as *mut _ as usize,
            )
        };
        result_from_retval(ret)?;

        Ok(thread_id)
    }

    /// 等待进程退出
    pub fn wait(&self) -> Result<i32> {
        self.wait_timeout(u64::MAX)
    }

    /// 带超时等待
    pub fn wait_timeout(&self, timeout_ns: u64) -> Result<i32> {
        let mut exit_code: i32 = 0;

        let ret = unsafe {
            syscall::syscall3(
                nr::SYS_PROCESS_WAIT,
                self.handle.raw() as usize,
                &mut exit_code as *mut _ as usize,
                timeout_ns as usize,
            )
        };
        result_from_retval(ret)?;

        Ok(exit_code)
    }
}

/// 进程构建器
pub struct ProcessBuilder {
    name: Vec<u8>,
    create_bootstrap: bool,
    init_handles: Vec<(Handle, Rights)>,
}

impl ProcessBuilder {
    fn new(name: &str) -> Self {
        Self {
            name: name.as_bytes().to_vec(),
            create_bootstrap: true,
            init_handles: Vec::new(),
        }
    }

    /// 是否创建 bootstrap channel
    pub fn bootstrap(mut self, create: bool) -> Self {
        self.create_bootstrap = create;
        self
    }

    /// 添加初始句柄
    pub fn add_handle(mut self, handle: Handle, rights: Rights) -> Self {
        self.init_handles.push((handle, rights));
        self
    }

    /// 创建进程（不启动）
    pub fn build(self) -> Result<Process> {
        let options = ProcessCreateOptions {
            name_ptr: self.name.as_ptr() as usize,
            name_len: self.name.len(),
            create_bootstrap: self.create_bootstrap,
        };

        let mut result = ProcessCreateResult {
            process_handle: 0,
            bootstrap_handle: 0,
        };

        let ret = unsafe {
            syscall::syscall2(
                nr::SYS_PROCESS_CREATE,
                &options as *const _ as usize,
                &mut result as *mut _ as usize,
            )
        };
        result_from_retval(ret)?;

        let bootstrap = if result.bootstrap_handle != 0 {
            Some(Channel::from_handle(OwnedHandle::from_raw(
                result.bootstrap_handle,
            )))
        } else {
            None
        };

        Ok(Process {
            handle: OwnedHandle::from_raw(result.process_handle),
            bootstrap,
        })
    }

    /// 创建并启动进程
    pub fn spawn(self) -> Result<Process> {
        let process = self.build()?;
        process.start()?;
        Ok(process)
    }
}

/// 在当前进程创建线程
pub fn spawn_thread(name: &str, entry: usize, stack_top: usize, arg: usize) -> Result<u32> {
    let options = ThreadCreateOptions {
        process_handle: 0, // 当前进程
        name_ptr: name.as_ptr() as usize,
        name_len: name.len(),
        entry,
        stack_top,
        arg,
    };

    let mut thread_id: u32 = 0;

    let ret = unsafe {
        syscall::syscall2(
            nr::SYS_THREAD_CREATE,
            &options as *const _ as usize,
            &mut thread_id as *mut _ as usize,
        )
    };
    result_from_retval(ret)?;

    Ok(thread_id)
}

/// 获取 bootstrap channel
pub fn get_bootstrap_channel() -> Result<Channel> {
    let ret = unsafe { syscall::syscall1(nr::SYS_PROCESS_GET_INIT_HANDLE, 0) };
    let handle = result_from_retval(ret)? as u32;

    if handle == 0 {
        Err(Error::new(EINVAL))
    } else {
        let mut handle = OwnedHandle::from_raw(handle);
        handle.with_nodrop(true);
        Ok(Channel::from_handle(handle))
    }
}

/// 获取初始句柄
pub fn get_init_handle(index: usize) -> Result<Handle> {
    let ret = unsafe { syscall::syscall1(nr::SYS_PROCESS_GET_INIT_HANDLE, index + 1) };
    let handle = result_from_retval(ret)? as u32;

    if handle == 0 {
        Err(Error::new(EINVAL))
    } else {
        Ok(Handle::from_raw(handle))
    }
}

/// 退出当前进程
pub fn exit(code: i32) -> ! {
    unsafe {
        syscall::syscall1(nr::SYS_EXIT, code as usize);
    }
    loop {
        core::hint::spin_loop();
    }
}

/// 让出 CPU
pub fn yield_now() {
    unsafe {
        syscall::syscall0(nr::SYS_YIELD);
    }
}
