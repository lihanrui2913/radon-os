// kernel/src/syscall/memory.rs

use alloc::sync::Arc;
use rmm::{PhysicalAddress, VirtualAddress};

use crate::{
    EPERM,
    object::{
        Handle, KernelObject, Rights,
        process::current_process,
        vmar::{MappingFlags, Vmar, VmarError},
        vmo::{Vmo, VmoError, VmoOptions},
    },
};

use super::error::{EACCES, EBADF, EEXIST, EINVAL, ENOENT, ENOMEM, Error, Result};

/// VMO 创建参数
#[repr(C)]
#[derive(Debug)]
pub struct VmoCreateArgs {
    /// 大小
    pub size: usize,
    /// 选项
    pub options: u32,
}

/// 创建 VMO
pub fn sys_vmo_create(args_ptr: usize, handle_out: usize) -> Result<usize> {
    if args_ptr == 0 || handle_out == 0 {
        return Err(Error::new(EINVAL));
    }

    let args = unsafe { &*(args_ptr as *const VmoCreateArgs) };
    let options = VmoOptions::from_bits_truncate(args.options);

    let vmo = Vmo::create(args.size, options).map_err(|e| match e {
        VmoError::InvalidSize => Error::new(EINVAL),
        VmoError::NoMemory => Error::new(ENOMEM),
        _ => Error::new(EINVAL),
    })?;

    let process = current_process().ok_or(Error::new(EINVAL))?;
    let mut proc = process.write();

    let handle = proc.handles_mut().insert(
        vmo as Arc<dyn KernelObject>,
        Rights::BASIC | Rights::MAP | Rights::DUPLICATE | Rights::TRANSFER,
    );

    unsafe {
        *(handle_out as *mut u32) = handle.raw();
    }

    Ok(0)
}

/// 创建物理内存 VMO
pub fn sys_vmo_create_physical(phys_addr: usize, size: usize, handle_out: usize) -> Result<usize> {
    // 需要特权检查
    // TODO: 检查调用者是否有权限创建物理 VMO

    if handle_out == 0 {
        return Err(Error::new(EINVAL));
    }

    let vmo = Vmo::create_physical(PhysicalAddress::new(phys_addr), size)
        .map_err(|_| Error::new(EINVAL))?;

    let process = current_process().ok_or(Error::new(EINVAL))?;
    let mut proc = process.write();

    let handle = proc.handles_mut().insert(
        vmo as Arc<dyn KernelObject>,
        Rights::BASIC | Rights::MAP | Rights::DUPLICATE | Rights::TRANSFER,
    );

    unsafe {
        *(handle_out as *mut u32) = handle.raw();
    }

    Ok(0)
}

/// 创建 VMO 子对象（COW 克隆）
pub fn sys_vmo_create_child(
    vmo_handle: usize,
    offset: usize,
    size: usize,
    handle_out: usize,
) -> Result<usize> {
    if handle_out == 0 {
        return Err(Error::new(EINVAL));
    }

    let vmo_arc = {
        let process = current_process().ok_or(Error::new(EINVAL))?;
        let proc = process.read();

        proc.handles()
            .get(Handle::from(vmo_handle), Rights::DUPLICATE)
            .ok_or(Error::new(EBADF))?
    };

    let _vmo = vmo_arc
        .as_any()
        .downcast_ref::<Vmo>()
        .ok_or(Error::new(EINVAL))?;

    // 这里需要 Arc<Vmo>，需要特殊处理
    let vmo_arc_typed = unsafe {
        let ptr = Arc::as_ptr(&vmo_arc) as *const Vmo;
        Arc::increment_strong_count(ptr);
        Arc::from_raw(ptr)
    };

    let child = vmo_arc_typed
        .create_cow_clone(offset, size)
        .map_err(|e| match e {
            VmoError::OutOfRange => Error::new(EINVAL),
            VmoError::NoMemory => Error::new(ENOMEM),
            _ => Error::new(EINVAL),
        })?;

    let process = current_process().ok_or(Error::new(EINVAL))?;
    let mut proc = process.write();

    let handle = proc.handles_mut().insert(
        child as Arc<dyn KernelObject>,
        Rights::BASIC | Rights::MAP | Rights::DUPLICATE | Rights::TRANSFER,
    );

    unsafe {
        *(handle_out as *mut u32) = handle.raw();
    }

    Ok(0)
}

/// 读取 VMO
pub fn sys_vmo_read(
    vmo_handle: usize,
    offset: usize,
    buf_ptr: usize,
    buf_len: usize,
) -> Result<usize> {
    if buf_ptr == 0 || buf_len == 0 {
        return Err(Error::new(EINVAL));
    }

    let vmo_obj = {
        let process = current_process().ok_or(Error::new(EINVAL))?;
        let proc = process.read();

        proc.handles()
            .get(Handle::from(vmo_handle), Rights::READ)
            .ok_or(Error::new(EBADF))?
    };

    let vmo = vmo_obj
        .as_any()
        .downcast_ref::<Vmo>()
        .ok_or(Error::new(EINVAL))?;

    let buf = unsafe { core::slice::from_raw_parts_mut(buf_ptr as *mut u8, buf_len) };

    let bytes_read = vmo.read(offset, buf).map_err(|_| Error::new(EINVAL))?;

    Ok(bytes_read)
}

/// 写入 VMO
pub fn sys_vmo_write(
    vmo_handle: usize,
    offset: usize,
    buf_ptr: usize,
    buf_len: usize,
) -> Result<usize> {
    if buf_ptr == 0 || buf_len == 0 {
        return Err(Error::new(EINVAL));
    }

    let vmo_obj = {
        let process = current_process().ok_or(Error::new(EINVAL))?;
        let proc = process.read();

        proc.handles()
            .get(Handle::from(vmo_handle), Rights::WRITE)
            .ok_or(Error::new(EBADF))?
    };

    let vmo = vmo_obj
        .as_any()
        .downcast_ref::<Vmo>()
        .ok_or(Error::new(EINVAL))?;

    let buf = unsafe { core::slice::from_raw_parts(buf_ptr as *const u8, buf_len) };

    let bytes_written = vmo.write(offset, buf).map_err(|_| Error::new(EINVAL))?;

    Ok(bytes_written)
}

/// 获取 VMO 大小
pub fn sys_vmo_get_size(vmo_handle: usize) -> Result<usize> {
    let vmo_obj = {
        let process = current_process().ok_or(Error::new(EINVAL))?;
        let proc = process.read();

        proc.handles()
            .get(Handle::from(vmo_handle), Rights::READ)
            .ok_or(Error::new(EBADF))?
    };

    let vmo = vmo_obj
        .as_any()
        .downcast_ref::<Vmo>()
        .ok_or(Error::new(EINVAL))?;

    Ok(vmo.size())
}

/// 设置 VMO 大小
pub fn sys_vmo_set_size(vmo_handle: usize, size: usize) -> Result<usize> {
    let vmo_obj = {
        let process = current_process().ok_or(Error::new(EINVAL))?;
        let proc = process.read();

        proc.handles()
            .get(Handle::from(vmo_handle), Rights::READ)
            .ok_or(Error::new(EBADF))?
    };

    let vmo = vmo_obj
        .as_any()
        .downcast_ref::<Vmo>()
        .ok_or(Error::new(EINVAL))?;

    vmo.resize(size).map_err(|_| Error::new(EPERM))?;

    Ok(0)
}

/// 获取 VMO 物理地址
pub fn sys_vmo_get_phys(vmo_handle: usize) -> Result<usize> {
    let vmo_obj = {
        let process = current_process().ok_or(Error::new(EINVAL))?;
        let proc = process.read();

        proc.handles()
            .get(Handle::from(vmo_handle), Rights::READ)
            .ok_or(Error::new(EBADF))?
    };

    let vmo = vmo_obj
        .as_any()
        .downcast_ref::<Vmo>()
        .ok_or(Error::new(EINVAL))?;

    vmo.get_physical()
}

/// 映射参数
#[repr(C)]
#[derive(Debug)]
pub struct VmarMapArgs {
    /// VMAR 句柄（0 表示进程根 VMAR）
    pub vmar_handle: u32,
    /// VMO 句柄
    pub vmo_handle: u32,
    /// VMO 内偏移
    pub vmo_offset: usize,
    /// 映射大小
    pub size: usize,
    /// 映射标志
    pub flags: u32,
    /// 指定地址（如果 flags 包含 SPECIFIC）
    pub vaddr: usize,
}

/// 映射 VMO 到地址空间
pub fn sys_vmar_map(args_ptr: usize, addr_out: usize) -> Result<usize> {
    if args_ptr == 0 || addr_out == 0 {
        return Err(Error::new(EINVAL));
    }

    let args = unsafe { &*(args_ptr as *const VmarMapArgs) };
    let flags = MappingFlags::from_bits_truncate(args.flags);

    let process = current_process().ok_or(Error::new(EINVAL))?;

    // 获取 VMAR
    let vmar = if args.vmar_handle == 0 {
        // 使用进程根 VMAR
        process.read().root_vmar().ok_or(Error::new(EINVAL))?
    } else {
        let proc = process.read();
        let obj = proc
            .handles()
            .get(Handle::from(args.vmar_handle as usize), Rights::MAP)
            .ok_or(Error::new(EBADF))?;

        obj.as_any()
            .downcast_ref::<Vmar>()
            .ok_or(Error::new(EINVAL))?;

        // 需要 Arc<Vmar>
        unsafe {
            let ptr = Arc::as_ptr(&obj) as *const Vmar;
            Arc::increment_strong_count(ptr);
            Arc::from_raw(ptr)
        }
    };

    // 获取 VMO
    let vmo = {
        let proc = process.read();
        let obj = proc
            .handles()
            .get(Handle::from(args.vmo_handle as usize), Rights::MAP)
            .ok_or(Error::new(EBADF))?;

        obj.as_any()
            .downcast_ref::<Vmo>()
            .ok_or(Error::new(EINVAL))?;

        unsafe {
            let ptr = Arc::as_ptr(&obj) as *const Vmo;
            Arc::increment_strong_count(ptr);
            Arc::from_raw(ptr)
        }
    };

    // 执行映射
    let vaddr = if flags.contains(MappingFlags::SPECIFIC) && (args.vaddr != 0) {
        Some(VirtualAddress::new(args.vaddr))
    } else {
        None
    };

    let mapped_addr = vmar
        .map(vmo, args.vmo_offset, args.size, flags, vaddr)
        .map_err(|e| match e {
            VmarError::NoSpace => Error::new(ENOMEM),
            VmarError::Overlap => Error::new(EEXIST),
            VmarError::OutOfRange => Error::new(EINVAL),
            _ => Error::new(EINVAL),
        })?;

    unsafe {
        *(addr_out as *mut usize) = mapped_addr.data();
    }

    Ok(0)
}

/// 解除映射
pub fn sys_vmar_unmap(vmar_handle: usize, addr: usize, size: usize) -> Result<usize> {
    let process = current_process().ok_or(Error::new(EINVAL))?;

    let vmar = if vmar_handle == 0 {
        process.read().root_vmar().ok_or(Error::new(EINVAL))?
    } else {
        let proc = process.read();
        let obj = proc
            .handles()
            .get(Handle::from(vmar_handle), Rights::MAP)
            .ok_or(Error::new(EBADF))?;

        unsafe {
            let ptr = Arc::as_ptr(&obj) as *const Vmar;
            Arc::increment_strong_count(ptr);
            Arc::from_raw(ptr)
        }
    };

    vmar.unmap(VirtualAddress::new(addr), size)
        .map_err(|e| match e {
            VmarError::NotMapped => Error::new(ENOENT),
            _ => Error::new(EINVAL),
        })?;

    Ok(0)
}

/// 修改映射权限
pub fn sys_vmar_protect(
    vmar_handle: usize,
    addr: usize,
    size: usize,
    flags: usize,
) -> Result<usize> {
    let process = current_process().ok_or(Error::new(EINVAL))?;

    let vmar = if vmar_handle == 0 {
        process.read().root_vmar().ok_or(Error::new(EINVAL))?
    } else {
        let proc = process.read();
        let obj = proc
            .handles()
            .get(Handle::from(vmar_handle), Rights::MAP)
            .ok_or(Error::new(EBADF))?;

        unsafe {
            let ptr = Arc::as_ptr(&obj) as *const Vmar;
            Arc::increment_strong_count(ptr);
            Arc::from_raw(ptr)
        }
    };

    vmar.protect(
        VirtualAddress::new(addr),
        size,
        MappingFlags::from_bits_truncate(flags as u32),
    )
    .map_err(|e| match e {
        VmarError::NotMapped => Error::new(ENOENT),
        VmarError::AccessDenied => Error::new(EACCES),
        _ => Error::new(EINVAL),
    })?;

    Ok(0)
}
