//! Name Server 通信协议

use core::mem::size_of;

/// 协议魔数
pub const PROTOCOL_MAGIC: u32 = 0x4E53_5652; // "NSVR"

/// 协议版本
pub const PROTOCOL_VERSION: u32 = 1;

/// 最大服务名长度
pub const MAX_SERVICE_NAME_LEN: usize = 256;

/// 最大描述长度
pub const MAX_DESCRIPTION_LEN: usize = 512;

/// 操作码
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpCode {
    /// 注册服务
    Register = 1,
    /// 注销服务
    Unregister = 2,
    /// 查找服务
    Lookup = 3,
    /// 列出服务
    List = 4,
    /// 检查服务是否存在
    Exists = 5,

    /// 监视服务（当服务上线/下线时通知）
    Watch = 10,
    /// 取消监视
    Unwatch = 11,

    /// 连接到服务（返回与服务通信的 Channel）
    Connect = 20,

    /// 获取服务信息
    GetInfo = 30,
    /// 更新服务信息
    UpdateInfo = 31,

    /// 服务上线通知
    NotifyOnline = 100,
    /// 服务下线通知
    NotifyOffline = 101,
}

impl From<u32> for OpCode {
    fn from(v: u32) -> Self {
        match v {
            1 => OpCode::Register,
            2 => OpCode::Unregister,
            3 => OpCode::Lookup,
            4 => OpCode::List,
            5 => OpCode::Exists,
            10 => OpCode::Watch,
            11 => OpCode::Unwatch,
            20 => OpCode::Connect,
            30 => OpCode::GetInfo,
            31 => OpCode::UpdateInfo,
            100 => OpCode::NotifyOnline,
            101 => OpCode::NotifyOffline,
            _ => OpCode::Lookup, // 默认
        }
    }
}

/// 状态码
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// 成功
    Ok = 0,
    /// 服务已存在
    AlreadyExists = -1,
    /// 服务不存在
    NotFound = -2,
    /// 权限不足
    PermissionDenied = -3,
    /// 无效参数
    InvalidArgument = -4,
    /// 服务不可用
    ServiceUnavailable = -5,
    /// 超时
    Timeout = -6,
    /// 内部错误
    InternalError = -7,
    /// 名称太长
    NameTooLong = -8,
    /// 资源不足
    ResourceExhausted = -9,
}

impl From<i32> for Status {
    fn from(v: i32) -> Self {
        match v {
            0 => Status::Ok,
            -1 => Status::AlreadyExists,
            -2 => Status::NotFound,
            -3 => Status::PermissionDenied,
            -4 => Status::InvalidArgument,
            -5 => Status::ServiceUnavailable,
            -6 => Status::Timeout,
            -7 => Status::InternalError,
            -8 => Status::NameTooLong,
            -9 => Status::ResourceExhausted,
            _ => Status::InternalError,
        }
    }
}

/// 消息头
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct MessageHeader {
    /// 魔数
    pub magic: u32,
    /// 版本
    pub version: u32,
    /// 操作码
    pub opcode: u32,
    /// 标志
    pub flags: u32,
    /// 序列号（用于匹配请求和响应）
    pub sequence: u32,
    /// 状态码（响应时使用）
    pub status: i32,
    /// 数据长度
    pub data_len: u32,
    /// 句柄数量
    pub handle_count: u32,
}

impl MessageHeader {
    pub const SIZE: usize = size_of::<Self>();

    pub fn new_request(opcode: OpCode, sequence: u32) -> Self {
        Self {
            magic: PROTOCOL_MAGIC,
            version: PROTOCOL_VERSION,
            opcode: opcode as u32,
            flags: MessageFlags::REQUEST.bits(),
            sequence,
            status: 0,
            data_len: 0,
            handle_count: 0,
        }
    }

    pub fn new_response(sequence: u32, status: Status) -> Self {
        Self {
            magic: PROTOCOL_MAGIC,
            version: PROTOCOL_VERSION,
            opcode: 0,
            flags: MessageFlags::RESPONSE.bits(),
            sequence,
            status: status as i32,
            data_len: 0,
            handle_count: 0,
        }
    }

    pub fn new_notification(opcode: OpCode) -> Self {
        Self {
            magic: PROTOCOL_MAGIC,
            version: PROTOCOL_VERSION,
            opcode: opcode as u32,
            flags: MessageFlags::NOTIFICATION.bits(),
            sequence: 0,
            status: 0,
            data_len: 0,
            handle_count: 0,
        }
    }

    pub fn is_valid(&self) -> bool {
        self.magic == PROTOCOL_MAGIC && self.version == PROTOCOL_VERSION
    }

    pub fn opcode(&self) -> OpCode {
        OpCode::from(self.opcode)
    }

    pub fn status(&self) -> Status {
        Status::from(self.status)
    }

    pub fn to_bytes(&self) -> [u8; Self::SIZE] {
        unsafe { core::mem::transmute(*self) }
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < Self::SIZE {
            return None;
        }
        let header: Self = unsafe { core::ptr::read_unaligned(bytes.as_ptr() as *const Self) };
        if header.is_valid() {
            Some(header)
        } else {
            None
        }
    }
}

bitflags::bitflags! {
    /// 消息标志
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct MessageFlags: u32 {
        const REQUEST = 1 << 0;
        const RESPONSE = 1 << 1;
        const NOTIFICATION = 1 << 2;
        const NEED_ACK = 1 << 3;
    }
}

bitflags::bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    pub struct ServiceFlags: u32 {
        /// 独占服务（只能有一个实例）
        const EXCLUSIVE = 1 << 0;
        /// 持久服务（不会自动注销）
        const PERSISTENT = 1 << 1;
        /// 系统服务（需要特权）
        const SYSTEM = 1 << 2;
        /// 隐藏服务（不在 list 中显示）
        const HIDDEN = 1 << 3;
        /// 多实例服务
        const MULTI_INSTANCE = 1 << 4;
    }
}

/// 注册请求
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct RegisterRequest {
    /// 服务标志
    pub flags: u32,
    /// 服务名长度
    pub name_len: u32,
    /// 描述长度
    pub desc_len: u32,
    /// 保留
    pub reserved: u32,
    // 后跟: name bytes, description bytes
}

/// 注册响应
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct RegisterResponse {
    /// 分配的服务 ID
    pub service_id: u64,
}

/// 查找/连接请求
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct LookupRequest {
    /// 服务名长度
    pub name_len: u32,
    /// 超时（毫秒，0 = 不等待）
    pub timeout_ms: u32,
    // 后跟: name bytes
}

/// 查找响应
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct LookupResponse {
    /// 服务 ID
    pub service_id: u64,
    /// 服务标志
    pub flags: u32,
    /// 保留
    pub reserved: u32,
    // 响应还包含一个 Channel handle
}

/// 服务信息
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct ServiceInfo {
    /// 服务 ID
    pub service_id: u64,
    /// 服务标志
    pub flags: u32,
    /// 注册时间（Unix 时间戳）
    pub registered_at: u64,
    /// 连接数
    pub connection_count: u32,
    /// 服务名长度
    pub name_len: u32,
    /// 描述长度
    pub desc_len: u32,
    /// 所有者进程 ID
    pub owner_pid: u32,
    // 后跟: name bytes, description bytes
}

/// 列表请求
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct ListRequest {
    /// 最大数量
    pub limit: u32,
    /// 过滤器：前缀长度
    pub contain_name_len: u32,
    // 后跟: contain_name bytes (可选)
}

/// 列表响应
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct ListResponse {
    /// 总服务数
    pub total_count: u32,
    /// 返回的服务数
    pub returned_count: u32,
    // 后跟: ServiceInfo 数组
}

/// 监视请求
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct WatchRequest {
    /// 服务名长度（0 = 监视所有）
    pub name_len: u32,
    /// 监视事件类型
    pub events: u32,
    // 后跟: name bytes (可选)
}

bitflags::bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct WatchEvents: u32 {
        const ONLINE = 1 << 0;
        const OFFLINE = 1 << 1;
        const ALL = Self::ONLINE.bits() | Self::OFFLINE.bits();
    }
}

/// 通知消息
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct NotificationData {
    /// 服务 ID
    pub service_id: u64,
    /// 服务名长度
    pub name_len: u32,
    /// 保留
    pub reserved: u32,
    // 后跟: name bytes
}
