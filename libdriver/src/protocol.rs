//! 驱动通信协议定义

use alloc::vec::Vec;
use core::mem::size_of;
use libradon::handle::Handle;

/// 消息头
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct MessageHeader {
    /// 消息类型/操作码
    pub op: u32,
    /// 消息标志
    pub flags: MessageFlags,
    /// 请求 ID（用于匹配响应）
    pub request_id: u32,
    /// 数据长度
    pub data_len: u32,
    /// 句柄数量
    pub handle_count: u32,
    /// 状态/错误码（响应时使用）
    pub status: i32,
}

bitflags::bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct MessageFlags: u32 {
        /// 这是一个请求
        const REQUEST = 1 << 0;
        /// 这是一个响应
        const RESPONSE = 1 << 1;
        /// 需要响应
        const NEED_REPLY = 1 << 2;
        /// 单向消息（无需响应）
        const ONE_WAY = 1 << 3;
        /// 包含共享内存
        const HAS_BUFFER = 1 << 4;
        /// 紧急消息
        const URGENT = 1 << 5;
    }
}

impl MessageHeader {
    pub const SIZE: usize = size_of::<Self>();

    pub fn new_request(op: u32, request_id: u32) -> Self {
        Self {
            op,
            flags: MessageFlags::REQUEST | MessageFlags::NEED_REPLY,
            request_id,
            data_len: 0,
            handle_count: 0,
            status: 0,
        }
    }

    pub fn new_response(request_id: u32, status: i32) -> Self {
        Self {
            op: 0,
            flags: MessageFlags::RESPONSE,
            request_id,
            data_len: 0,
            handle_count: 0,
            status,
        }
    }

    pub fn new_oneway(op: u32) -> Self {
        Self {
            op,
            flags: MessageFlags::REQUEST | MessageFlags::ONE_WAY,
            request_id: 0,
            data_len: 0,
            handle_count: 0,
            status: 0,
        }
    }

    /// 序列化为字节
    pub fn to_bytes(&self) -> [u8; Self::SIZE] {
        unsafe { core::mem::transmute(*self) }
    }

    /// 从字节反序列化
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < Self::SIZE {
            return None;
        }
        Some(unsafe { core::ptr::read_unaligned(bytes.as_ptr() as *const Self) })
    }
}

/// 通用驱动操作码
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriverOp {
    /// 打开设备
    Open = 1,
    /// 关闭设备
    Close = 2,

    /// 读取
    Read = 10,
    /// 写入
    Write = 11,
    /// 控制命令
    Ioctl = 12,

    /// 获取共享缓冲区
    GetBuffer = 20,
    /// 释放共享缓冲区
    ReleaseBuffer = 21,
    /// 映射设备内存
    MapMmio = 22,

    /// 等待中断
    WaitIrq = 30,
    /// 确认中断
    AckIrq = 31,

    // 设备特定（256 以上
    /// 用户自定义起始
    UserDefined = 256,
}

impl From<u32> for DriverOp {
    fn from(v: u32) -> Self {
        match v {
            1 => DriverOp::Open,
            2 => DriverOp::Close,
            10 => DriverOp::Read,
            11 => DriverOp::Write,
            12 => DriverOp::Ioctl,
            20 => DriverOp::GetBuffer,
            21 => DriverOp::ReleaseBuffer,
            22 => DriverOp::MapMmio,
            30 => DriverOp::WaitIrq,
            31 => DriverOp::AckIrq,
            _ => DriverOp::UserDefined,
        }
    }
}

/// 请求消息
#[derive(Debug)]
pub struct Request {
    pub header: MessageHeader,
    pub data: Vec<u8>,
    pub handles: Vec<Handle>,
}

impl Request {
    pub fn new(op: DriverOp, request_id: u32) -> Self {
        Self {
            header: MessageHeader::new_request(op as u32, request_id),
            data: Vec::new(),
            handles: Vec::new(),
        }
    }

    pub fn with_data(mut self, data: Vec<u8>) -> Self {
        self.header.data_len = data.len() as u32;
        self.data = data;
        self
    }

    pub fn with_handles(mut self, handles: Vec<Handle>) -> Self {
        self.header.handle_count = handles.len() as u32;
        self.handles = handles;
        self
    }

    /// 编码为发送缓冲区
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(MessageHeader::SIZE + self.data.len());
        buf.extend_from_slice(&self.header.to_bytes());
        buf.extend_from_slice(&self.data);
        buf
    }
}

/// 响应消息
#[derive(Debug)]
pub struct Response {
    pub header: MessageHeader,
    pub data: Vec<u8>,
    pub handles: Vec<Handle>,
}

impl Response {
    pub fn success(request_id: u32) -> Self {
        Self {
            header: MessageHeader::new_response(request_id, 0),
            data: Vec::new(),
            handles: Vec::new(),
        }
    }

    pub fn error(request_id: u32, status: i32) -> Self {
        Self {
            header: MessageHeader::new_response(request_id, status),
            data: Vec::new(),
            handles: Vec::new(),
        }
    }

    pub fn with_data(mut self, data: Vec<u8>) -> Self {
        self.header.data_len = data.len() as u32;
        self.data = data;
        self
    }

    pub fn with_handles(mut self, handles: Vec<Handle>) -> Self {
        self.header.handle_count = handles.len() as u32;
        self.handles = handles;
        self
    }

    pub fn is_success(&self) -> bool {
        self.header.status == 0
    }

    /// 编码为发送缓冲区
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(MessageHeader::SIZE + self.data.len());
        buf.extend_from_slice(&self.header.to_bytes());
        buf.extend_from_slice(&self.data);
        buf
    }
}

/// 读写请求参数
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct IoRequest {
    /// 偏移
    pub offset: u64,
    /// 长度
    pub length: u32,
    /// 标志
    pub flags: u32,
}

/// Ioctl 请求
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct IoctlRequest {
    /// 命令码
    pub cmd: u32,
    /// 参数
    pub arg: u64,
}

/// 缓冲区请求
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct BufferRequest {
    /// 请求大小
    pub size: usize,
    /// 对齐要求
    pub alignment: usize,
    /// 标志
    pub flags: u32,
}

/// 缓冲区响应
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct BufferResponse {
    /// 物理地址（DMA 用）
    pub phys_addr: u64,
    /// 大小
    pub size: usize,
}
