use crate::syscall::{self, nr, result_from_retval};
use bitflags::bitflags;
use core::fmt;
use radon_kernel::{EBADF, Error, Result};

/// 句柄类型
#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Handle(u32);

impl Handle {
    /// 无效句柄
    pub const INVALID: Handle = Handle(0);

    /// 从原始值创建
    #[inline]
    pub const fn from_raw(raw: u32) -> Self {
        Handle(raw)
    }

    /// 获取原始值
    #[inline]
    pub const fn raw(&self) -> u32 {
        self.0
    }

    /// 是否有效
    #[inline]
    pub const fn is_valid(&self) -> bool {
        self.0 != 0
    }

    /// 关闭句柄
    pub fn close(self) -> Result<()> {
        if !self.is_valid() {
            return Err(Error::new(EBADF));
        }

        let ret = unsafe { syscall::syscall1(nr::SYS_HANDLE_CLOSE, self.0 as usize) };
        result_from_retval(ret).map(|_| ())
    }

    /// 复制句柄
    pub fn duplicate(&self, rights: Rights) -> Result<Handle> {
        if !self.is_valid() {
            return Err(Error::new(EBADF));
        }

        let ret = unsafe {
            syscall::syscall2(
                nr::SYS_HANDLE_DUPLICATE,
                self.0 as usize,
                rights.bits() as usize,
            )
        };
        result_from_retval(ret).map(|v| Handle(v as u32))
    }
}

impl fmt::Debug for Handle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Handle({})", self.0)
    }
}

impl fmt::Display for Handle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<u32> for Handle {
    fn from(v: u32) -> Self {
        Handle(v)
    }
}

impl From<Handle> for u32 {
    fn from(h: Handle) -> Self {
        h.0
    }
}

bitflags! {
    /// 句柄权限
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Rights: u32 {
        const READ      = 1 << 0;
        const WRITE     = 1 << 1;
        const EXECUTE   = 1 << 2;
        const MAP       = 1 << 3;
        const DUPLICATE = 1 << 4;
        const TRANSFER  = 1 << 5;
        const WAIT      = 1 << 6;
        const SIGNAL    = 1 << 7;
        const MANAGE    = 1 << 8;

        const BASIC = Self::READ.bits() | Self::WRITE.bits() | Self::WAIT.bits();
        const ALL = u32::MAX;
    }
}

/// 带自动关闭的句柄
pub struct OwnedHandle {
    handle: Handle,
    nodrop: bool,
}

impl OwnedHandle {
    /// 从原始句柄创建（获取所有权）
    #[inline]
    pub const fn from_raw(raw: u32) -> Self {
        Self {
            handle: Handle(raw),
            nodrop: false,
        }
    }

    /// 获取内部句柄（借用）
    #[inline]
    pub const fn handle(&self) -> Handle {
        self.handle
    }

    /// 获取原始值
    #[inline]
    pub const fn raw(&self) -> u32 {
        self.handle.0
    }

    /// 释放所有权，返回原始句柄
    #[inline]
    pub fn into_raw(self) -> u32 {
        let raw = self.handle.0;
        core::mem::forget(self);
        raw
    }

    #[inline]
    pub fn with_nodrop(&mut self, nodrop: bool) {
        self.nodrop = nodrop;
    }

    /// 关闭句柄
    pub fn close(self) -> Result<()> {
        self.handle.close()
    }

    /// 复制句柄
    pub fn duplicate(&self, rights: Rights) -> Result<OwnedHandle> {
        self.handle.duplicate(rights).map(|h| OwnedHandle {
            handle: h,
            nodrop: false,
        })
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        if self.handle.is_valid() && !self.nodrop {
            let _ = self.handle.close();
        }
    }
}

impl fmt::Debug for OwnedHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "OwnedHandle({})", self.handle.0)
    }
}

pub trait AsHandle {
    fn as_handle(&self) -> Handle;
}

impl AsHandle for Handle {
    fn as_handle(&self) -> Handle {
        *self
    }
}

impl AsHandle for OwnedHandle {
    fn as_handle(&self) -> Handle {
        self.handle
    }
}

impl<T: AsHandle> AsHandle for &T {
    fn as_handle(&self) -> Handle {
        (*self).as_handle()
    }
}
