//! Bootstrap 协议定义
//!
//! Bootstrap 是进程启动时获取基础服务 Channel 的机制。

use core::mem::size_of;

/// 协议魔数
pub const BOOTSTRAP_MAGIC: u32 = 0x424F_4F54; // "BOOT"

/// 最大服务名长度
pub const MAX_SERVICE_NAME: usize = 64;

/// Bootstrap 请求类型
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestType {
    /// 获取服务 Channel
    GetService = 1,
    /// 注册为服务提供者（仅限特权进程）
    RegisterProvider = 2,
    /// 列出可用服务
    ListServices = 3,
    /// 心跳/存活检查
    Ping = 4,
}

impl From<u32> for RequestType {
    fn from(v: u32) -> Self {
        match v {
            1 => RequestType::GetService,
            2 => RequestType::RegisterProvider,
            3 => RequestType::ListServices,
            4 => RequestType::Ping,
            _ => RequestType::GetService,
        }
    }
}

/// 响应状态
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponseStatus {
    Ok = 0,
    NotFound = -1,
    AlreadyExists = -2,
    PermissionDenied = -3,
    InvalidRequest = -4,
    ServiceUnavailable = -5,
}

impl From<i32> for ResponseStatus {
    fn from(v: i32) -> Self {
        match v {
            0 => ResponseStatus::Ok,
            -1 => ResponseStatus::NotFound,
            -2 => ResponseStatus::AlreadyExists,
            -3 => ResponseStatus::PermissionDenied,
            -4 => ResponseStatus::InvalidRequest,
            -5 => ResponseStatus::ServiceUnavailable,
            _ => ResponseStatus::InvalidRequest,
        }
    }
}

/// Bootstrap 请求头
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct BootstrapRequest {
    /// 魔数
    pub magic: u32,
    /// 请求类型
    pub request_type: u32,
    /// 服务名长度
    pub name_len: u32,
    /// 保留
    pub reserved: u32,
    // 后跟: 服务名字节
}

impl BootstrapRequest {
    pub const SIZE: usize = size_of::<Self>();

    pub fn new(request_type: RequestType, name_len: usize) -> Self {
        Self {
            magic: BOOTSTRAP_MAGIC,
            request_type: request_type as u32,
            name_len: name_len as u32,
            reserved: 0,
        }
    }

    pub fn is_valid(&self) -> bool {
        self.magic == BOOTSTRAP_MAGIC
    }

    pub fn request_type(&self) -> RequestType {
        RequestType::from(self.request_type)
    }

    pub fn to_bytes(&self) -> [u8; Self::SIZE] {
        unsafe { core::mem::transmute(*self) }
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < Self::SIZE {
            return None;
        }
        let req: Self = unsafe { core::ptr::read_unaligned(bytes.as_ptr() as *const Self) };
        if req.is_valid() { Some(req) } else { None }
    }
}

/// Bootstrap 响应头
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct BootstrapResponse {
    /// 魔数
    pub magic: u32,
    /// 状态码
    pub status: i32,
    /// 数据长度
    pub data_len: u32,
    /// 句柄数量
    pub handle_count: u32,
    // 响应可能包含 Channel handle
}

impl BootstrapResponse {
    pub const SIZE: usize = size_of::<Self>();

    pub fn success() -> Self {
        Self {
            magic: BOOTSTRAP_MAGIC,
            status: ResponseStatus::Ok as i32,
            data_len: 0,
            handle_count: 0,
        }
    }

    pub fn error(status: ResponseStatus) -> Self {
        Self {
            magic: BOOTSTRAP_MAGIC,
            status: status as i32,
            data_len: 0,
            handle_count: 0,
        }
    }

    pub fn with_handle(mut self) -> Self {
        self.handle_count = 1;
        self
    }

    pub fn is_success(&self) -> bool {
        self.status == 0
    }

    pub fn status(&self) -> ResponseStatus {
        ResponseStatus::from(self.status)
    }

    pub fn to_bytes(&self) -> [u8; Self::SIZE] {
        unsafe { core::mem::transmute(*self) }
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < Self::SIZE {
            return None;
        }
        Some(unsafe { core::ptr::read_unaligned(bytes.as_ptr() as *const Self) })
    }
}

/// 预定义的 Bootstrap 服务名
pub mod services {
    /// Name Server
    pub const NAMESERVER: &str = "NAMESERVER";
    /// 块设备驱动系统
    pub const BLOCKSERVER: &str = "BLOCKSERVER";
    /// 文件系统
    pub const FSSERVER: &str = "FSSERVER";
}
