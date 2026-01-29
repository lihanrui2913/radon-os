//! Name Server 客户端

use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};

use libradon::handle::OwnedHandle;
use libradon::port::{BindOptions, Deadline};
use libradon::{channel::Channel, handle::Handle, port::Port, port::PortPacket, signal::Signals};

use crate::protocol::*;
use crate::{Error, Result};

/// Name Server 客户端
pub struct NameService {
    /// 与 Name Server 通信的 Channel
    channel: Channel,
    /// 事件 Port
    port: Port,
    /// 下一个序列号
    next_seq: AtomicU32,
}

impl NameService {
    /// 连接到 Name Server
    pub fn connect() -> Result<Self> {
        // 使用 bootstrap 机制获取 Name Server channel
        let channel = bootstrap::get_nameserver().map_err(|e| match e {
            bootstrap::BootstrapError::ServiceNotFound => Error::ServiceUnavailable,
            bootstrap::BootstrapError::NoBootstrapChannel => Error::ServiceUnavailable,
            _ => Error::InternalError,
        })?;

        Self::from_channel(channel)
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
            next_seq: AtomicU32::new(1),
        })
    }

    /// 获取下一个序列号
    fn next_sequence(&self) -> u32 {
        self.next_seq.fetch_add(1, Ordering::Relaxed)
    }

    /// 发送请求并等待响应
    fn request(
        &self,
        opcode: OpCode,
        data: &[u8],
        handles: &[Handle],
        timeout: Deadline,
    ) -> Result<(MessageHeader, Vec<u8>, Vec<Handle>)> {
        let seq = self.next_sequence();

        // 构造请求
        let mut header = MessageHeader::new_request(opcode, seq);
        header.data_len = data.len() as u32;
        header.handle_count = handles.len() as u32;

        // 发送
        let mut req_buf = Vec::with_capacity(MessageHeader::SIZE + data.len());
        req_buf.extend_from_slice(&header.to_bytes());
        req_buf.extend_from_slice(data);

        self.channel.send_with_handles(&req_buf, handles)?;

        // 等待响应
        self.wait_response(seq, timeout)
    }

    /// 等待响应
    fn wait_response(
        &self,
        sequence: u32,
        timeout: Deadline,
    ) -> Result<(MessageHeader, Vec<u8>, Vec<Handle>)> {
        let mut packets = [PortPacket::zeroed(); 4];
        let mut recv_buf = vec![0u8; 4096];
        let mut recv_handles = [Handle::INVALID; 16];

        loop {
            // 尝试接收
            match self.channel.try_recv(&mut recv_buf, &mut recv_handles) {
                Ok(result) if result.data_len >= MessageHeader::SIZE => {
                    let header =
                        MessageHeader::from_bytes(&recv_buf).ok_or(Error::InvalidArgument)?;

                    if header.sequence == sequence {
                        if header.status != 0 {
                            return Err(Status::from(header.status).into());
                        }

                        let data = recv_buf
                            [MessageHeader::SIZE..MessageHeader::SIZE + header.data_len as usize]
                            .to_vec();
                        let handles = recv_handles[..result.handle_count]
                            .iter()
                            .copied()
                            .collect();

                        return Ok((header, data, handles));
                    }
                    // 不是我们要的响应，继续等待
                }
                Ok(_) => {}
                Err(e) if e.errno == radon_kernel::EAGAIN => {}
                Err(e) if e.errno == radon_kernel::EPIPE => {
                    return Err(Error::Disconnected);
                }
                Err(e) => return Err(e.into()),
            }

            // 等待事件
            let count = self.port.wait(&mut packets, timeout)?;

            if count == 0 {
                return Err(Error::Timeout);
            }

            for packet in &packets[..count] {
                if packet.signals.contains(Signals::PEER_CLOSED) {
                    return Err(Error::Disconnected);
                }
            }
        }
    }

    /// 注册服务
    pub fn register(
        &self,
        name: &str,
        description: &str,
        flags: ServiceFlags,
        service_channel: &Channel,
    ) -> Result<ServiceHandle> {
        if name.len() > MAX_SERVICE_NAME_LEN {
            return Err(Error::NameTooLong);
        }

        // 构造请求数据
        let req = RegisterRequest {
            flags: flags.bits(),
            name_len: name.len() as u32,
            desc_len: description.len() as u32,
            reserved: 0,
        };

        let mut data = Vec::with_capacity(
            core::mem::size_of::<RegisterRequest>() + name.len() + description.len(),
        );
        data.extend_from_slice(unsafe {
            core::slice::from_raw_parts(
                &req as *const _ as *const u8,
                core::mem::size_of::<RegisterRequest>(),
            )
        });
        data.extend_from_slice(name.as_bytes());
        data.extend_from_slice(description.as_bytes());

        // 发送请求
        let (_header, resp_data, _) = self.request(
            OpCode::Register,
            &data,
            &[service_channel.handle()],
            Deadline::Infinite,
        )?;

        // 解析响应
        if resp_data.len() >= core::mem::size_of::<RegisterResponse>() {
            let resp: &RegisterResponse =
                unsafe { &*(resp_data.as_ptr() as *const RegisterResponse) };

            Ok(ServiceHandle {
                service_id: resp.service_id,
                name: name.to_string(),
            })
        } else {
            Err(Error::InternalError)
        }
    }

    /// 注销服务
    pub fn unregister(&self, name: &str) -> Result<()> {
        if name.len() > MAX_SERVICE_NAME_LEN {
            return Err(Error::NameTooLong);
        }

        let mut data = Vec::with_capacity(4 + name.len());
        data.extend_from_slice(&(name.len() as u32).to_le_bytes());
        data.extend_from_slice(name.as_bytes());

        let _ = self.request(OpCode::Unregister, &data, &[], Deadline::Infinite)?;

        Ok(())
    }

    /// 查找服务
    pub fn lookup(&self, name: &str) -> Result<ServiceInfo> {
        self.lookup_timeout(name, 0)
    }

    /// 带超时查找服务
    pub fn lookup_timeout(&self, name: &str, timeout_ms: u32) -> Result<ServiceInfo> {
        if name.len() > MAX_SERVICE_NAME_LEN {
            return Err(Error::NameTooLong);
        }

        let req = LookupRequest {
            name_len: name.len() as u32,
            timeout_ms,
        };

        let mut data = Vec::with_capacity(core::mem::size_of::<LookupRequest>() + name.len());
        data.extend_from_slice(unsafe {
            core::slice::from_raw_parts(
                &req as *const _ as *const u8,
                core::mem::size_of::<LookupRequest>(),
            )
        });
        data.extend_from_slice(name.as_bytes());

        let deadline = if timeout_ms == 0 {
            Deadline::Infinite
        } else {
            Deadline::Relative(timeout_ms as u64 * 1_000_000) // ms -> ns
        };

        let (_, resp_data, _) = self.request(OpCode::Lookup, &data, &[], deadline)?;

        Self::parse_service_info(&resp_data)
    }

    /// 连接到服务
    pub fn connect_to(&self, name: &str) -> Result<Channel> {
        self.connect_timeout(name, 0)
    }

    /// 带超时连接到服务
    pub fn connect_timeout(&self, name: &str, timeout_ms: u32) -> Result<Channel> {
        if name.len() > MAX_SERVICE_NAME_LEN {
            return Err(Error::NameTooLong);
        }

        let req = LookupRequest {
            name_len: name.len() as u32,
            timeout_ms,
        };

        let mut data = Vec::with_capacity(core::mem::size_of::<LookupRequest>() + name.len());
        data.extend_from_slice(unsafe {
            core::slice::from_raw_parts(
                &req as *const _ as *const u8,
                core::mem::size_of::<LookupRequest>(),
            )
        });
        data.extend_from_slice(name.as_bytes());

        let deadline = if timeout_ms == 0 {
            Deadline::Infinite
        } else {
            Deadline::Relative(timeout_ms as u64 * 1_000_000)
        };

        let (_, _, handles) = self.request(OpCode::Connect, &data, &[], deadline)?;

        if handles.is_empty() {
            return Err(Error::ServiceUnavailable);
        }

        Ok(Channel::from_handle(OwnedHandle::from_raw(
            handles[0].raw(),
        )))
    }

    /// 检查服务是否存在
    pub fn exists(&self, name: &str) -> Result<bool> {
        match self.lookup(name) {
            Ok(_) => Ok(true),
            Err(Error::NotFound) => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// 列出服务
    pub fn list(&self, prefix: Option<&str>, offset: u32, limit: u32) -> Result<Vec<ServiceInfo>> {
        let prefix = prefix.unwrap_or("");

        let req = ListRequest {
            offset,
            limit,
            prefix_len: prefix.len() as u32,
            reserved: 0,
        };

        let mut data = Vec::with_capacity(core::mem::size_of::<ListRequest>() + prefix.len());
        data.extend_from_slice(unsafe {
            core::slice::from_raw_parts(
                &req as *const _ as *const u8,
                core::mem::size_of::<ListRequest>(),
            )
        });
        data.extend_from_slice(prefix.as_bytes());

        let (_, resp_data, _) = self.request(OpCode::List, &data, &[], Deadline::Infinite)?;

        Self::parse_service_list(&resp_data)
    }

    /// 监视服务
    pub fn watch(&self, name: Option<&str>, events: WatchEvents) -> Result<WatchHandle> {
        let name = name.unwrap_or("");

        let req = WatchRequest {
            name_len: name.len() as u32,
            events: events.bits(),
        };

        let mut data = Vec::with_capacity(core::mem::size_of::<WatchRequest>() + name.len());
        data.extend_from_slice(unsafe {
            core::slice::from_raw_parts(
                &req as *const _ as *const u8,
                core::mem::size_of::<WatchRequest>(),
            )
        });
        data.extend_from_slice(name.as_bytes());

        let (header, _, _) = self.request(OpCode::Watch, &data, &[], Deadline::Infinite)?;

        Ok(WatchHandle {
            watch_id: header.sequence, // 使用序列号作为 watch ID
            pattern: if name.is_empty() {
                None
            } else {
                Some(name.to_string())
            },
        })
    }

    /// 取消监视
    pub fn unwatch(&self, handle: WatchHandle) -> Result<()> {
        let data = handle.watch_id.to_le_bytes();

        let _ = self.request(OpCode::Unwatch, &data, &[], Deadline::Infinite)?;

        Ok(())
    }

    /// 等待通知
    pub fn wait_notification(&self, timeout: Deadline) -> Result<Notification> {
        let mut packets = [PortPacket::zeroed(); 4];
        let mut recv_buf = vec![0u8; 1024];

        loop {
            // 等待事件
            let count = self.port.wait(&mut packets, timeout)?;

            if count == 0 {
                return Err(Error::Timeout);
            }

            for packet in &packets[..count] {
                if packet.signals.contains(Signals::PEER_CLOSED) {
                    return Err(Error::Disconnected);
                }

                if packet.signals.contains(Signals::READABLE) {
                    // 尝试接收通知
                    let mut handles = [Handle::INVALID; 4];
                    if let Ok(result) = self.channel.try_recv(&mut recv_buf, &mut handles) {
                        if result.data_len >= MessageHeader::SIZE {
                            let header = MessageHeader::from_bytes(&recv_buf)
                                .ok_or(Error::InvalidArgument)?;

                            if header.flags & MessageFlags::NOTIFICATION.bits() != 0 {
                                return Self::parse_notification(&header, &recv_buf);
                            }
                        }
                    }
                }
            }
        }
    }

    fn parse_service_info(data: &[u8]) -> Result<ServiceInfo> {
        if data.len() < core::mem::size_of::<ServiceInfo>() {
            return Err(Error::InvalidArgument);
        }

        Ok(unsafe { core::ptr::read(data.as_ptr() as *const ServiceInfo) })
    }

    fn parse_service_list(data: &[u8]) -> Result<Vec<ServiceInfo>> {
        if data.len() < core::mem::size_of::<ListResponse>() {
            return Err(Error::InvalidArgument);
        }

        let resp: &ListResponse = unsafe { &*(data.as_ptr() as *const ListResponse) };

        let mut services = Vec::with_capacity(resp.returned_count as usize);
        let mut offset = core::mem::size_of::<ListResponse>();

        for _ in 0..resp.returned_count {
            if offset + core::mem::size_of::<ServiceInfo>() > data.len() {
                break;
            }

            let info: ServiceInfo =
                unsafe { core::ptr::read((data.as_ptr() as usize + offset) as *const ServiceInfo) };

            offset += core::mem::size_of::<ServiceInfo>()
                + info.name_len as usize
                + info.desc_len as usize;

            services.push(info);
        }

        Ok(services)
    }

    fn parse_notification(header: &MessageHeader, data: &[u8]) -> Result<Notification> {
        let payload_start = MessageHeader::SIZE;
        let payload = &data[payload_start..payload_start + header.data_len as usize];

        if payload.len() < core::mem::size_of::<NotificationData>() {
            return Err(Error::InvalidArgument);
        }

        let notif_data: &NotificationData =
            unsafe { &*(payload.as_ptr() as *const NotificationData) };

        let name_start = core::mem::size_of::<NotificationData>();
        let name_bytes = &payload[name_start..name_start + notif_data.name_len as usize];
        let name = core::str::from_utf8(name_bytes)
            .map_err(|_| Error::InvalidArgument)?
            .to_string();

        let event = match header.opcode() {
            OpCode::NotifyOnline => NotificationEvent::Online,
            OpCode::NotifyOffline => NotificationEvent::Offline,
            _ => return Err(Error::InvalidArgument),
        };

        Ok(Notification {
            event,
            service_id: notif_data.service_id,
            service_name: name,
        })
    }
}

/// 服务句柄（注册后返回）
#[derive(Debug)]
pub struct ServiceHandle {
    pub service_id: u64,
    pub name: String,
}

/// 监视句柄
#[derive(Debug)]
pub struct WatchHandle {
    pub watch_id: u32,
    pub pattern: Option<String>,
}

/// 通知事件
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationEvent {
    Online,
    Offline,
}

/// 通知
#[derive(Debug)]
pub struct Notification {
    pub event: NotificationEvent,
    pub service_id: u64,
    pub service_name: String,
}

/// 连接到服务（便捷函数）
pub fn connect(name: &str) -> Result<Channel> {
    let ns = NameService::connect()?;
    ns.connect_to(name)
}

/// 注册服务（便捷函数）
pub fn register(name: &str, channel: &Channel) -> Result<ServiceHandle> {
    let ns = NameService::connect()?;
    ns.register(name, "", ServiceFlags::empty(), channel)
}

/// 查找服务（便捷函数）
pub fn lookup(name: &str) -> Result<ServiceInfo> {
    let ns = NameService::connect()?;
    ns.lookup(name)
}
