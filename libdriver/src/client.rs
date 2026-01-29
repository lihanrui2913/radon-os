//! 驱动客户端框架

use alloc::format;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};
use radon_kernel::Error;

use libradon::port::{BindOptions, Deadline};

use libradon::{
    channel::Channel,
    handle::Handle,
    port::{Port, PortPacket},
    signal::Signals,
};

use crate::protocol::{DriverOp, MessageHeader, Request, Response};
use crate::{DriverError, Result};

/// 驱动客户端
pub struct DriverClient {
    channel: Channel,
    port: Port,
    next_request_id: AtomicU32,
}

impl DriverClient {
    /// 连接到驱动服务
    pub fn connect(service_name: &str) -> Result<Self> {
        let name = format!("driver.{}", service_name);
        while nameserver::client::lookup(&name).is_err() {
            libradon::process::yield_now();
        }

        let service_channel = nameserver::client::connect(&name).map_err(|e| Error::from(e))?;

        let port = Port::create()?;
        port.bind(
            1,
            &service_channel,
            Signals::READABLE | Signals::PEER_CLOSED,
            BindOptions::Persistent,
        )?;

        Ok(Self {
            channel: service_channel,
            port,
            next_request_id: AtomicU32::new(1),
        })
    }

    /// 从现有 Channel 创建客户端
    pub fn from_channel(channel: Channel) -> Result<Self> {
        let port = Port::create()?;
        port.bind(
            1,
            &channel,
            Signals::READABLE | Signals::PEER_CLOSED,
            BindOptions::Persistent,
        )?;

        Ok(Self {
            channel,
            port,
            next_request_id: AtomicU32::new(1),
        })
    }

    /// 分配请求 ID
    fn alloc_request_id(&self) -> u32 {
        self.next_request_id.fetch_add(1, Ordering::Relaxed)
    }

    /// 发送请求并等待响应
    pub fn call(&self, op: DriverOp, data: &[u8]) -> Result<Response> {
        self.call_with_handles(op, data, &[])
    }

    /// 发送请求（带句柄）并等待响应
    pub fn call_with_handles(
        &self,
        op: DriverOp,
        data: &[u8],
        handles: &[Handle],
    ) -> Result<Response> {
        let request_id = self.alloc_request_id();

        // 构造请求
        let request = Request::new(op, request_id)
            .with_data(data.to_vec())
            .with_handles(handles.to_vec());

        // 发送请求
        let req_data = request.encode();
        self.channel.send_with_handles(&req_data, handles)?;

        // 等待响应
        self.wait_response(request_id, Deadline::Infinite)
    }

    /// 发送单向请求（无需响应）
    pub fn send(&self, op: DriverOp, data: &[u8]) -> Result<()> {
        self.send_with_handles(op, data, &[])
    }

    /// 发送单向请求（带句柄）
    pub fn send_with_handles(&self, op: DriverOp, data: &[u8], handles: &[Handle]) -> Result<()> {
        let mut header = MessageHeader::new_oneway(op as u32);
        header.data_len = data.len() as u32;
        header.handle_count = handles.len() as u32;

        let mut buf = Vec::with_capacity(MessageHeader::SIZE + data.len());
        buf.extend_from_slice(&header.to_bytes());
        buf.extend_from_slice(data);

        self.channel.send_with_handles(&buf, handles)?;

        Ok(())
    }

    /// 等待响应
    fn wait_response(&self, request_id: u32, deadline: Deadline) -> Result<Response> {
        let mut packets = [PortPacket::zeroed(); 4];
        let mut recv_buf = [0u8; 4096];
        let mut recv_handles = [Handle::INVALID; 16];

        loop {
            // 先尝试接收
            match self
                .channel
                .try_recv_with_handles(&mut recv_buf, &mut recv_handles)
            {
                Ok(result) if result.data_len >= MessageHeader::SIZE => {
                    let header = MessageHeader::from_bytes(&recv_buf[..MessageHeader::SIZE])
                        .ok_or(DriverError::InvalidArgument)?;

                    if header.request_id == request_id {
                        let data_end = MessageHeader::SIZE + header.data_len as usize;
                        let data = recv_buf[MessageHeader::SIZE..data_end].to_vec();
                        let handles = recv_handles[..result.handle_count]
                            .iter()
                            .map(|h| *h)
                            .collect();

                        return Ok(Response {
                            header,
                            data,
                            handles,
                        });
                    }
                    // 不是我们要的响应，可能需要缓存
                }
                Ok(_) => {}
                Err(e) if e.errno == radon_kernel::EAGAIN => {}
                Err(e) if e.errno == radon_kernel::EPIPE => {
                    return Err(DriverError::Disconnected);
                }
                Err(e) => return Err(e.into()),
            }

            // 等待事件
            let count = self.port.wait(&mut packets, deadline)?;

            if count == 0 {
                return Err(DriverError::Timeout);
            }

            for packet in &packets[..count] {
                if packet.signals.contains(Signals::PEER_CLOSED) {
                    return Err(DriverError::Disconnected);
                }
            }
        }
    }

    /// 获取底层 Channel
    pub fn channel(&self) -> &Channel {
        &self.channel
    }

    /// 获取 Port
    pub fn port(&self) -> &Port {
        &self.port
    }
}

/// RPC 风格客户端
pub struct RpcClient {
    client: DriverClient,
}

impl RpcClient {
    /// 创建 RPC 客户端
    pub fn new(client: DriverClient) -> Self {
        Self { client }
    }

    /// 连接到服务
    pub fn connect(service_name: &str) -> Result<Self> {
        Ok(Self {
            client: DriverClient::connect(service_name)?,
        })
    }

    /// 读取
    pub fn read(&self, offset: u64, buf: &mut [u8]) -> Result<usize> {
        use crate::protocol::IoRequest;

        let req = IoRequest {
            offset,
            length: buf.len() as u32,
            flags: 0,
        };

        let req_data = unsafe {
            core::slice::from_raw_parts(
                &req as *const _ as *const u8,
                core::mem::size_of::<IoRequest>(),
            )
        };

        let response = self.client.call(DriverOp::Read, req_data)?;

        if !response.is_success() {
            return Err(DriverError::IoError);
        }

        // 复制数据
        let copy_len = core::cmp::min(buf.len(), response.data.len());
        buf[..copy_len].copy_from_slice(&response.data[..copy_len]);

        Ok(copy_len)
    }

    /// 写入
    pub fn write(&self, offset: u64, data: &[u8]) -> Result<usize> {
        use crate::protocol::IoRequest;

        // 构造请求：IoRequest + 数据
        let req = IoRequest {
            offset,
            length: data.len() as u32,
            flags: 0,
        };

        let mut req_data = Vec::with_capacity(core::mem::size_of::<IoRequest>() + data.len());
        req_data.extend_from_slice(unsafe {
            core::slice::from_raw_parts(
                &req as *const _ as *const u8,
                core::mem::size_of::<IoRequest>(),
            )
        });
        req_data.extend_from_slice(data);

        let response = self.client.call(DriverOp::Write, &req_data)?;

        if !response.is_success() {
            return Err(DriverError::IoError);
        }

        // 解析响应
        if response.data.len() >= core::mem::size_of::<u32>() {
            let transferred = u32::from_le_bytes(response.data[..4].try_into().unwrap());
            Ok(transferred as usize)
        } else {
            Ok(data.len())
        }
    }

    /// ioctl
    pub fn ioctl(&self, cmd: u32, arg: u64) -> Result<u64> {
        use crate::protocol::IoctlRequest;

        let req = IoctlRequest { cmd, arg };

        let req_data = unsafe {
            core::slice::from_raw_parts(
                &req as *const _ as *const u8,
                core::mem::size_of::<IoctlRequest>(),
            )
        };

        let response = self.client.call(DriverOp::Ioctl, req_data)?;

        if !response.is_success() {
            return Err(DriverError::IoError);
        }

        if response.data.len() >= 8 {
            Ok(u64::from_le_bytes(response.data[..8].try_into().unwrap()))
        } else {
            Ok(0)
        }
    }

    /// 获取共享缓冲区
    pub fn get_buffer(&self, size: usize) -> Result<(Handle, u64)> {
        use crate::protocol::BufferRequest;

        let req = BufferRequest {
            size,
            alignment: 4096,
            flags: 0,
        };

        let req_data = unsafe {
            core::slice::from_raw_parts(
                &req as *const _ as *const u8,
                core::mem::size_of::<BufferRequest>(),
            )
        };

        let response = self.client.call(DriverOp::GetBuffer, req_data)?;

        if !response.is_success() {
            return Err(DriverError::OutOfMemory);
        }

        if response.handles.is_empty() {
            return Err(DriverError::InvalidArgument);
        }

        let phys_addr = if response.data.len() >= 8 {
            u64::from_le_bytes(response.data[..8].try_into().unwrap())
        } else {
            0
        };

        Ok((response.handles[0], phys_addr))
    }
}
