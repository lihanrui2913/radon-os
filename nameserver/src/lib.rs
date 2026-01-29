#![no_std]

extern crate alloc;

pub mod protocol;

#[cfg(feature = "client")]
pub mod client;

#[cfg(feature = "server")]
pub mod server;

pub use protocol::*;

#[cfg(feature = "client")]
pub use client::{NameService, ServiceHandle, WatchHandle};
use radon_kernel::{
    EEXIST, EINVAL, ENAMETOOLONG, ENETUNREACH, ENOENT, ENOMEM, EPERM, EPIPE, EWOULDBLOCK,
};

/// Name Server 错误
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// 服务已存在
    AlreadyExists,
    /// 服务不存在
    NotFound,
    /// 权限不足
    PermissionDenied,
    /// 无效参数
    InvalidArgument,
    /// 服务不可用
    ServiceUnavailable,
    /// 超时
    Timeout,
    /// 内部错误
    InternalError,
    /// 名称太长
    NameTooLong,
    /// 资源不足
    ResourceExhausted,
    /// 连接断开
    Disconnected,
    /// 系统错误
    SystemError(i32),
}

impl From<Status> for Error {
    fn from(s: Status) -> Self {
        match s {
            Status::Ok => panic!("Ok is not an error"),
            Status::AlreadyExists => Error::AlreadyExists,
            Status::NotFound => Error::NotFound,
            Status::PermissionDenied => Error::PermissionDenied,
            Status::InvalidArgument => Error::InvalidArgument,
            Status::ServiceUnavailable => Error::ServiceUnavailable,
            Status::Timeout => Error::Timeout,
            Status::InternalError => Error::InternalError,
            Status::NameTooLong => Error::NameTooLong,
            Status::ResourceExhausted => Error::ResourceExhausted,
        }
    }
}

impl From<radon_kernel::Error> for Error {
    fn from(e: radon_kernel::Error) -> Self {
        Error::SystemError(e.errno)
    }
}

impl From<Error> for radon_kernel::Error {
    fn from(value: Error) -> Self {
        match value {
            Error::AlreadyExists => radon_kernel::Error::new(EEXIST),
            Error::NotFound => radon_kernel::Error::new(ENOENT),
            Error::PermissionDenied => radon_kernel::Error::new(EPERM),
            Error::InvalidArgument => radon_kernel::Error::new(EINVAL),
            Error::ServiceUnavailable => radon_kernel::Error::new(ENETUNREACH),
            Error::Timeout => radon_kernel::Error::new(EWOULDBLOCK),
            Error::InternalError => radon_kernel::Error::new(EINVAL),
            Error::NameTooLong => radon_kernel::Error::new(ENAMETOOLONG),
            Error::ResourceExhausted => radon_kernel::Error::new(ENOMEM),
            Error::Disconnected => radon_kernel::Error::new(EPIPE),
            Error::SystemError(e) => radon_kernel::Error::new(e),
        }
    }
}

pub type Result<T> = core::result::Result<T, Error>;
