//! Bootstrap 客户端
//!
//! 用于子进程从 init 获取基础服务的 Channel。

use alloc::vec::Vec;

use libradon::handle::OwnedHandle;
use libradon::process::get_bootstrap_channel;
use libradon::{channel::Channel, handle::Handle};

use crate::protocol::*;

/// Bootstrap 错误
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootstrapError {
    /// 无法获取 bootstrap channel
    NoBootstrapChannel,
    /// 服务不存在
    ServiceNotFound,
    /// 权限不足
    PermissionDenied,
    /// 无效响应
    InvalidResponse,
    /// 通信错误
    CommunicationError,
    /// 服务不可用
    ServiceUnavailable,
}

pub type Result<T> = core::result::Result<T, BootstrapError>;

/// Bootstrap 客户端
pub struct BootstrapClient {
    channel: Channel,
}

impl BootstrapClient {
    /// 连接到 bootstrap 服务（通常是 init 进程）
    pub fn connect() -> Result<Self> {
        let channel = get_bootstrap_channel().map_err(|_| BootstrapError::NoBootstrapChannel)?;

        Ok(Self { channel })
    }

    /// 从现有 channel 创建
    pub fn from_channel(channel: Channel) -> Self {
        Self { channel }
    }

    /// 获取服务 Channel
    pub fn get_service(&self, name: &str) -> Result<Channel> {
        if name.len() > MAX_SERVICE_NAME {
            return Err(BootstrapError::InvalidResponse);
        }

        // 构造请求
        let request = BootstrapRequest::new(RequestType::GetService, name.len());

        let mut buf = Vec::with_capacity(BootstrapRequest::SIZE + name.len());
        buf.extend_from_slice(&request.to_bytes());
        buf.extend_from_slice(name.as_bytes());

        // 发送请求
        self.channel
            .send(&buf)
            .map_err(|_| BootstrapError::CommunicationError)?;

        // 接收响应
        let mut resp_buf = [0u8; 256];
        let mut handles = [Handle::INVALID; 4];

        let result = self
            .channel
            .recv_with_handles(&mut resp_buf, &mut handles)
            .map_err(|_| BootstrapError::CommunicationError)?;

        // 解析响应
        let response =
            BootstrapResponse::from_bytes(&resp_buf).ok_or(BootstrapError::InvalidResponse)?;

        match response.status() {
            ResponseStatus::Ok => {
                if result.handle_count > 0 && handles[0].is_valid() {
                    let mut handle = OwnedHandle::from_raw(handles[0].raw());
                    handle.with_nodrop(true);
                    Ok(Channel::from_handle(handle))
                } else {
                    Err(BootstrapError::InvalidResponse)
                }
            }
            ResponseStatus::NotFound => Err(BootstrapError::ServiceNotFound),
            ResponseStatus::PermissionDenied => Err(BootstrapError::PermissionDenied),
            ResponseStatus::ServiceUnavailable => Err(BootstrapError::ServiceUnavailable),
            _ => Err(BootstrapError::InvalidResponse),
        }
    }

    /// 获取 Name Server
    pub fn get_nameserver(&self) -> Result<Channel> {
        self.get_service(services::NAMESERVER)
    }

    /// 获取块设备服务
    pub fn get_blockserver(&self) -> Result<Channel> {
        self.get_service(services::BLOCKSERVER)
    }

    /// 获取文件系统服务
    pub fn get_fsserver(&self) -> Result<Channel> {
        self.get_service(services::FSSERVER)
    }

    /// 注册为服务提供者（仅限特权进程）
    pub fn register_provider(&self, name: &str, channel: &Channel) -> Result<()> {
        if name.len() > MAX_SERVICE_NAME {
            return Err(BootstrapError::InvalidResponse);
        }

        let request = BootstrapRequest::new(RequestType::RegisterProvider, name.len());

        let mut buf = Vec::with_capacity(BootstrapRequest::SIZE + name.len());
        buf.extend_from_slice(&request.to_bytes());
        buf.extend_from_slice(name.as_bytes());

        // 发送请求和 channel
        self.channel
            .send_with_handles(&buf, &[channel.handle()])
            .map_err(|_| BootstrapError::CommunicationError)?;

        // 接收响应
        let mut resp_buf = [0u8; 64];
        self.channel
            .recv(&mut resp_buf)
            .map_err(|_| BootstrapError::CommunicationError)?;

        let response =
            BootstrapResponse::from_bytes(&resp_buf).ok_or(BootstrapError::InvalidResponse)?;

        match response.status() {
            ResponseStatus::Ok => Ok(()),
            ResponseStatus::AlreadyExists => Err(BootstrapError::InvalidResponse),
            ResponseStatus::PermissionDenied => Err(BootstrapError::PermissionDenied),
            _ => Err(BootstrapError::InvalidResponse),
        }
    }

    /// Ping（检查 init 是否存活）
    pub fn ping(&self) -> Result<()> {
        let request = BootstrapRequest::new(RequestType::Ping, 0);

        self.channel
            .send(&request.to_bytes())
            .map_err(|_| BootstrapError::CommunicationError)?;

        let mut resp_buf = [0u8; 64];
        self.channel
            .recv(&mut resp_buf)
            .map_err(|_| BootstrapError::CommunicationError)?;

        let response =
            BootstrapResponse::from_bytes(&resp_buf).ok_or(BootstrapError::InvalidResponse)?;

        if response.is_success() {
            Ok(())
        } else {
            Err(BootstrapError::CommunicationError)
        }
    }
}

/// 获取 Name Server Channel（便捷函数）
pub fn get_nameserver() -> Result<Channel> {
    BootstrapClient::connect()?.get_nameserver()
}

/// 获取指定服务 Channel（便捷函数）
pub fn get_service(name: &str) -> Result<Channel> {
    BootstrapClient::connect()?.get_service(name)
}
