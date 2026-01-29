use alloc::string::ToString;
use alloc::sync::Arc;
use rmm::{Arch, FrameAllocator, VirtualAddress};
use spin::RwLock;

use crate::{
    EAGAIN,
    arch::{CurrentRmmArch, irq::IrqRegsArch},
    init::memory::{FRAME_ALLOCATOR, PAGE_SIZE},
    layout,
    loader::{LoaderError, ProgramLoader},
    object::{
        Handle, KernelObject, Process, Rights, Signals,
        process::{current_process, register_process},
        vmar::Vmar,
    },
};

use super::error::{EBADF, EINVAL, ENOMEM, Error, Result};

/// 进程创建选项
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct ProcessCreateOptions {
    /// 进程名指针
    pub name_ptr: usize,
    /// 进程名长度
    pub name_len: usize,
    /// 是否创建 bootstrap channel
    pub create_bootstrap: bool,
}

#[repr(C)]
#[derive(Debug)]
pub struct ProcessCreateResult {
    /// 进程句柄
    pub process_handle: u32,
    /// Bootstrap channel 句柄（父进程端）
    pub bootstrap_handle: u32,
}

/// 创建进程
pub fn sys_process_create(options_ptr: usize, result_ptr: usize) -> Result<usize> {
    if options_ptr == 0 || result_ptr == 0 {
        return Err(Error::new(EINVAL));
    }

    let options = unsafe { (options_ptr as *const ProcessCreateOptions).as_ref_unchecked() };
    let result = unsafe { (result_ptr as *mut ProcessCreateResult).as_mut_unchecked() };

    // 获取进程名
    let name = if options.name_ptr != 0 && options.name_len > 0 {
        let name_slice =
            unsafe { core::slice::from_raw_parts(options.name_ptr as *const u8, options.name_len) };
        core::str::from_utf8(name_slice)
            .map_err(|_| Error::new(EINVAL))?
            .to_string()
    } else {
        "unnamed".to_string()
    };

    // 获取当前进程作为父进程
    let parent = current_process();

    // 创建新进程
    let (new_process, bootstrap_parent) = if options.create_bootstrap {
        Process::new_with_bootstrap(name, parent.clone())
    } else {
        (Process::new(name, parent.clone()), None)
    };

    let new_page_table = unsafe { FRAME_ALLOCATOR.lock().allocate_one() }
        .ok_or(LoaderError::OutOfMemory)
        .expect("No enougth memory to create new process");
    let new_page_table_virt = unsafe { CurrentRmmArch::phys_to_virt(new_page_table) };
    unsafe { core::ptr::write_bytes(new_page_table_virt.data() as *mut u8, 0, PAGE_SIZE) };
    unsafe { ProgramLoader::copy_kernel_mappings(new_page_table) }.unwrap();

    let user_base = VirtualAddress::new(layout::USER_SPACE_START);
    let user_size = layout::USER_SPACE_END - layout::USER_SPACE_START;
    new_process.write().set_root_vmar(Vmar::create_root(
        user_base,
        user_size,
        layout::ALLOC_START,
        new_page_table,
    ));

    // 注册进程
    register_process(new_process.clone());

    // 将进程对象添加到父进程的句柄表
    let process_handle = if let Some(parent) = parent {
        parent.write().handles_mut().insert(
            new_process.clone() as Arc<dyn KernelObject>,
            Rights::BASIC | Rights::MANAGE,
        )
    } else {
        // 无父进程（init 进程），直接返回 PID
        Handle::from_raw(new_process.read().pid() as u32)
    };

    // 处理 bootstrap channel
    let bootstrap_handle = if let Some(parent_channel) = bootstrap_parent {
        if let Some(parent) = current_process() {
            parent.write().handles_mut().insert(
                parent_channel as Arc<dyn KernelObject>,
                Rights::BASIC | Rights::TRANSFER,
            )
        } else {
            Handle::INVALID
        }
    } else {
        Handle::INVALID
    };

    result.process_handle = process_handle.raw();
    result.bootstrap_handle = bootstrap_handle.raw();

    Ok(0)
}

/// 线程创建选项
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct ThreadCreateOptions {
    /// 进程句柄（0 表示当前进程）
    pub process_handle: u32,
    /// 线程名指针
    pub name_ptr: usize,
    /// 线程名长度
    pub name_len: usize,
    /// 入口地址
    pub entry: usize,
    /// 栈顶地址
    pub stack_top: usize,
    /// 参数
    pub arg: usize,
}

/// 在进程中创建线程
pub fn sys_thread_create(options_ptr: usize, thread_handle_out: usize) -> Result<usize> {
    if options_ptr == 0 {
        return Err(Error::new(EINVAL));
    }

    let options = unsafe { (options_ptr as *const ThreadCreateOptions).as_ref_unchecked() };

    // 获取目标进程
    let process = if options.process_handle == 0 {
        current_process().ok_or(Error::new(EINVAL))?
    } else {
        let current = current_process().ok_or(Error::new(EINVAL))?;

        // 获取 Arc
        current
            .read()
            .handles()
            .get_unchecked(Handle::from_raw(options.process_handle))
            .ok_or(Error::new(EBADF))?
    };

    let process = process
        .as_any()
        .downcast_ref::<RwLock<Process>>()
        .ok_or(Error::new(EINVAL))?;

    // 获取线程名
    let name = if options.name_ptr != 0 && options.name_len > 0 {
        let name_slice =
            unsafe { core::slice::from_raw_parts(options.name_ptr as *const u8, options.name_len) };
        core::str::from_utf8(name_slice)
            .map_err(|_| Error::new(EINVAL))?
            .to_string()
    } else {
        "thread".to_string()
    };

    // 创建线程
    let task = {
        let mut proc = process.write();
        if proc.main_thread().is_none() {
            proc.create_main_thread(options.entry, options.stack_top)
        } else {
            proc.create_thread(name, options.entry, options.stack_top)
        }
    }
    .ok_or(Error::new(ENOMEM))?;

    // 设置参数
    if options.arg != 0 {
        let t = task.read();
        // 设置第一个参数
        let regs = unsafe { t.pt_regs().as_mut().unwrap() };
        regs.set_args((options.arg as u64, 0, 0, 0, 0, 0));
    }

    // 返回线程 ID 或句柄
    if thread_handle_out != 0 {
        unsafe {
            *(thread_handle_out as *mut u32) = task.read().tid() as u32;
        }
    }

    Ok(task.read().tid())
}

pub fn sys_process_start(process_handle: usize) -> Result<usize> {
    let process = if process_handle == 0 {
        return Err(Error::new(EINVAL));
    } else {
        let current = current_process().ok_or(Error::new(EINVAL))?;

        current
            .read()
            .handles()
            .get_unchecked(Handle::from_raw(process_handle as u32))
            .ok_or(Error::new(EBADF))?
    };

    if let Ok(proc) = Arc::downcast::<RwLock<Process>>(process.clone()) {
        proc.write().start();
        Ok(0)
    } else {
        Err(Error::new(EINVAL))
    }
}

pub fn sys_process_get_init_handle(index: usize) -> Result<usize> {
    let process = current_process().ok_or(Error::new(EINVAL))?;
    let proc = process.read();

    if index == 0 {
        // 返回 bootstrap channel
        if let Some(handle) = proc.bootstrap_handle() {
            return Ok(handle.raw() as usize);
        }
    } else {
        // 返回第 index 个初始句柄
        let init_handles = proc.init_handles();
        if index <= init_handles.len() {
            return Ok(init_handles[index - 1].raw() as usize);
        }
    }

    Err(Error::new(EBADF))
}

pub fn sys_process_get_vmar_handle(process_handle: usize) -> Result<usize> {
    let process = current_process().ok_or(Error::new(EINVAL))?;

    let process = if process_handle == 0 {
        process
    } else {
        process
            .read()
            .handles()
            .get_unchecked(Handle::from_raw(process_handle as u32))
            .ok_or(Error::new(EBADF))?
    };

    if let Ok(proc) = Arc::downcast::<RwLock<Process>>(process.clone()) {
        let root_vmar = proc.read().root_vmar().ok_or(Error::new(EINVAL))?;
        let handle = current_process()
            .ok_or(Error::new(EINVAL))?
            .write()
            .handles_mut()
            .insert(
                root_vmar as Arc<dyn KernelObject>,
                Rights::BASIC | Rights::MANAGE | Rights::MAP,
            );

        Ok(handle.raw() as usize)
    } else {
        Err(Error::new(EINVAL))
    }
}

/// 退出当前进程
pub fn sys_exit(exit_code: usize) -> Result<usize> {
    let code = exit_code as i32;

    // if let Some(process) = current_process() {
    //     process.write().exit(code);
    // }

    crate::task::exit_current(code);
}

#[allow(unused)]
pub fn sys_process_wait(
    process_handle: usize,
    exit_code_out: usize,
    timeout_ns: usize,
) -> Result<usize> {
    let current = current_process().ok_or(Error::new(EINVAL))?;

    let process_obj = current
        .read()
        .handles()
        .get(Handle::from_raw(process_handle as u32), Rights::WAIT)
        .ok_or(Error::new(EBADF))?;

    // 检查进程是否已退出
    if process_obj.signals().contains(Signals::TERMINATED) {
        if exit_code_out != 0 {
            // 获取退出码
            // 需要类型转换
            if let Some(proc) = process_obj.as_any().downcast_ref::<RwLock<Process>>() {
                let code = proc.read().exit_code();
                unsafe {
                    *(exit_code_out as *mut i32) = code;
                }
            }
        }
        return Ok(0);
    }

    // TODO: 实现等待（使用 Port 或 WaitQueue）
    Err(Error::new(EAGAIN))
}
