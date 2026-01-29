use crate::handle::{Handle, OwnedHandle};
use crate::syscall::{self, nr, result_from_retval};
use bitflags::bitflags;
use radon_kernel::Result;

bitflags! {
    /// VMO 创建选项
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct VmoOptions: u32 {
        /// 立即分配物理内存（默认是按需分配）
        const COMMIT = 1 << 0;
        /// 物理连续（用于 DMA）
        const CONTIGUOUS = 1 << 1;
        /// 可调整大小
        const RESIZABLE = 1 << 2;
        /// 可丢弃（内存压力时可被回收）
        const DISCARDABLE = 1 << 3;
    }
}

bitflags! {
    /// 映射标志
    #[derive(Debug, Clone, Copy)]
    pub struct MappingFlags: u32 {
        const READ = 1 << 0;
        const WRITE = 1 << 1;
        const EXECUTE = 1 << 2;
        const SPECIFIC = 1 << 3;
    }
}

/// Virtual Memory Object
pub struct Vmo {
    handle: OwnedHandle,
}

impl Vmo {
    /// 创建 VMO
    pub fn create(size: usize, options: VmoOptions) -> Result<Self> {
        #[repr(C)]
        struct Args {
            size: usize,
            options: u32,
        }

        let args = Args {
            size,
            options: options.bits(),
        };

        let mut handle: u32 = 0;

        let ret = unsafe {
            syscall::syscall2(
                nr::SYS_VMO_CREATE,
                &args as *const _ as usize,
                &mut handle as *mut _ as usize,
            )
        };
        result_from_retval(ret)?;

        Ok(Self {
            handle: OwnedHandle::from_raw(handle),
        })
    }

    pub fn create_physical(addr: usize, size: usize) -> Result<Self> {
        let mut handle: u32 = 0;

        let ret = unsafe {
            syscall::syscall3(
                nr::SYS_VMO_CREATE_PHYSICAL,
                addr as usize,
                size,
                &mut handle as *mut _ as usize,
            )
        };
        result_from_retval(ret)?;

        Ok(Self {
            handle: OwnedHandle::from_raw(handle),
        })
    }

    /// 获取句柄
    pub fn handle(&self) -> Handle {
        self.handle.handle()
    }

    /// 读取数据
    pub fn read(&self, offset: usize, buf: &mut [u8]) -> Result<usize> {
        let ret = unsafe {
            syscall::syscall4(
                nr::SYS_VMO_READ,
                self.handle.raw() as usize,
                offset,
                buf.as_mut_ptr() as usize,
                buf.len(),
            )
        };
        result_from_retval(ret)
    }

    /// 写入数据
    pub fn write(&self, offset: usize, buf: &[u8]) -> Result<usize> {
        let ret = unsafe {
            syscall::syscall4(
                nr::SYS_VMO_WRITE,
                self.handle.raw() as usize,
                offset,
                buf.as_ptr() as usize,
                buf.len(),
            )
        };
        result_from_retval(ret)
    }

    /// 获取大小
    pub fn size(&self) -> Result<usize> {
        let ret = unsafe { syscall::syscall1(nr::SYS_VMO_GET_SIZE, self.handle.raw() as usize) };
        result_from_retval(ret)
    }

    /// 创建 COW 克隆
    pub fn create_child(&self, offset: usize, size: usize) -> Result<Vmo> {
        let mut handle: u32 = 0;

        let ret = unsafe {
            syscall::syscall4(
                nr::SYS_VMO_CREATE_CHILD,
                self.handle.raw() as usize,
                offset,
                size,
                &mut handle as *mut _ as usize,
            )
        };
        result_from_retval(ret)?;

        Ok(Vmo {
            handle: OwnedHandle::from_raw(handle),
        })
    }
}

/// 映射 VMO 到当前进程地址空间
pub fn map_vmo(vmo: &Vmo, vmo_offset: usize, size: usize, flags: MappingFlags) -> Result<*mut u8> {
    #[repr(C)]
    struct Args {
        vmar_handle: u32,
        vmo_handle: u32,
        vmo_offset: usize,
        size: usize,
        flags: u32,
        vaddr: usize,
    }

    let args = Args {
        vmar_handle: 0, // 根 VMAR
        vmo_handle: vmo.handle().raw(),
        vmo_offset,
        size,
        flags: flags.bits(),
        vaddr: 0,
    };

    let mut addr: usize = 0;

    let ret = unsafe {
        syscall::syscall2(
            nr::SYS_VMAR_MAP,
            &args as *const _ as usize,
            &mut addr as *mut _ as usize,
        )
    };
    result_from_retval(ret)?;

    Ok(addr as *mut u8)
}

/// 映射 VMO 到指定地址
pub fn map_vmo_at(
    vmo: &Vmo,
    vmo_offset: usize,
    size: usize,
    flags: MappingFlags,
    vaddr: *mut u8,
) -> Result<()> {
    #[repr(C)]
    struct Args {
        vmar_handle: u32,
        vmo_handle: u32,
        vmo_offset: usize,
        size: usize,
        flags: u32,
        vaddr: usize,
    }

    let args = Args {
        vmar_handle: 0,
        vmo_handle: vmo.handle().raw(),
        vmo_offset,
        size,
        flags: (flags | MappingFlags::SPECIFIC).bits(),
        vaddr: vaddr as usize,
    };

    let mut addr: usize = 0;

    let ret = unsafe {
        syscall::syscall2(
            nr::SYS_VMAR_MAP,
            &args as *const _ as usize,
            &mut addr as *mut _ as usize,
        )
    };
    result_from_retval(ret)?;

    Ok(())
}

pub fn map_vmo_in_vmar(
    vmar_handle: Handle,
    vmo: &Vmo,
    vmo_offset: usize,
    size: usize,
    flags: MappingFlags,
) -> Result<*mut u8> {
    #[repr(C)]
    struct Args {
        vmar_handle: u32,
        vmo_handle: u32,
        vmo_offset: usize,
        size: usize,
        flags: u32,
        vaddr: usize,
    }

    let args = Args {
        vmar_handle: vmar_handle.raw(),
        vmo_handle: vmo.handle().raw(),
        vmo_offset,
        size,
        flags: flags.bits(),
        vaddr: 0,
    };

    let mut addr: usize = 0;

    let ret = unsafe {
        syscall::syscall2(
            nr::SYS_VMAR_MAP,
            &args as *const _ as usize,
            &mut addr as *mut _ as usize,
        )
    };
    result_from_retval(ret)?;

    Ok(addr as *mut u8)
}

pub fn map_vmo_at_in_vmar(
    vmar_handle: Handle,
    vmo: &Vmo,
    vmo_offset: usize,
    size: usize,
    flags: MappingFlags,
    vaddr: *mut u8,
) -> Result<*mut u8> {
    #[repr(C)]
    struct Args {
        vmar_handle: u32,
        vmo_handle: u32,
        vmo_offset: usize,
        size: usize,
        flags: u32,
        vaddr: usize,
    }

    let args = Args {
        vmar_handle: vmar_handle.raw(),
        vmo_handle: vmo.handle().raw(),
        vmo_offset,
        size,
        flags: (flags | MappingFlags::SPECIFIC).bits(),
        vaddr: vaddr as usize,
    };

    let mut addr: usize = 0;

    let ret = unsafe {
        syscall::syscall2(
            nr::SYS_VMAR_MAP,
            &args as *const _ as usize,
            &mut addr as *mut _ as usize,
        )
    };
    result_from_retval(ret)?;

    Ok(addr as *mut u8)
}

/// 解除映射
pub fn unmap(addr: *mut u8, size: usize) -> Result<()> {
    let ret = unsafe {
        syscall::syscall3(
            nr::SYS_VMAR_UNMAP,
            0, // 根 VMAR
            addr as usize,
            size,
        )
    };
    result_from_retval(ret)?;
    Ok(())
}

/// 修改映射权限
pub fn protect(addr: *mut u8, size: usize, flags: MappingFlags) -> Result<()> {
    let ret = unsafe {
        syscall::syscall4(
            nr::SYS_VMAR_PROTECT,
            0,
            addr as usize,
            size,
            flags.bits() as usize,
        )
    };
    result_from_retval(ret)?;
    Ok(())
}
