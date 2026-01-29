#![no_std]
#![feature(allocator_api)]

extern crate alloc;

pub mod buffer;
pub mod client;
pub mod dma;
pub mod irq;
pub mod mmio;
pub mod protocol;
pub mod ring;
pub mod server;

// 重新导出常用类型
pub use buffer::{BufferPool, SharedBuffer};
pub use client::{DriverClient, RpcClient};
pub use dma::{DmaBuffer, DmaPool, DmaRegion, PhysAddr};
pub use irq::{IrqHandler, IrqToken};
pub use mmio::MmioRegion;
pub use protocol::{DriverOp, MessageHeader, Request, Response};
pub use ring::{Descriptor, RingBuffer};
pub use server::{DriverServer, RequestHandler, ServiceBuilder};

/// 驱动错误类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriverError {
    /// 无效参数
    InvalidArgument,
    /// 内存不足
    OutOfMemory,
    /// 无效句柄
    InvalidHandle,
    /// 连接断开
    Disconnected,
    /// 超时
    Timeout,
    /// 缓冲区太小
    BufferTooSmall,
    /// 设备忙
    DeviceBusy,
    /// IO 错误
    IoError,
    /// 权限不足
    PermissionDenied,
    /// 不支持的操作
    NotSupported,
    /// 系统错误
    SystemError(i32),
}

pub type Result<T> = core::result::Result<T, DriverError>;

impl From<radon_kernel::Error> for DriverError {
    fn from(e: radon_kernel::Error) -> Self {
        match e.errno {
            radon_kernel::EINVAL => DriverError::InvalidArgument,
            radon_kernel::ENOMEM => DriverError::OutOfMemory,
            radon_kernel::EBADF => DriverError::InvalidHandle,
            radon_kernel::EPIPE => DriverError::Disconnected,
            radon_kernel::ETIMEDOUT => DriverError::Timeout,
            radon_kernel::EACCES => DriverError::PermissionDenied,
            errno => DriverError::SystemError(errno),
        }
    }
}
