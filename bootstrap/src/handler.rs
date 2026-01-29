//! Bootstrap 请求处理器
//!
//! 在 init 进程中运行，处理子进程的 bootstrap 请求。

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use spin::RwLock;

use libradon::handle::OwnedHandle;
use libradon::port::{BindOptions, Deadline};
use libradon::{
    channel::Channel,
    handle::Handle,
    port::{Port, PortPacket},
    signal::Signals,
};

use crate::protocol::*;

/// 服务条目
struct ServiceEntry {
    /// 服务名
    name: String,
    /// 服务 Channel（init 持有的一端）
    channel: Channel,
    /// 是否为系统服务（需要特权）
    is_system: bool,
}

/// 子进程连接
struct ChildConnection {
    id: u64,
    channel: Channel,
    port_key: u64,
    /// 是否为特权进程
    privileged: bool,
}

/// Bootstrap 处理器
pub struct BootstrapHandler {
    /// 事件 Port
    port: Port,
    /// 注册的服务
    services: RwLock<BTreeMap<String, ServiceEntry>>,
    /// 子进程连接
    children: RwLock<BTreeMap<u64, ChildConnection>>,
    /// 下一个连接 ID
    next_conn_id: RwLock<u64>,
    /// 是否运行中
    running: RwLock<bool>,
}

impl BootstrapHandler {
    /// 创建新的处理器
    pub fn new() -> radon_kernel::Result<Self> {
        let port = Port::create()?;

        Ok(Self {
            port,
            services: RwLock::new(BTreeMap::new()),
            children: RwLock::new(BTreeMap::new()),
            next_conn_id: RwLock::new(1),
            running: RwLock::new(false),
        })
    }

    /// 预注册服务
    ///
    /// 在启动子进程之前，注册核心服务的 Channel。
    pub fn register_service(&self, name: &str, channel: Channel, is_system: bool) {
        self.services.write().insert(
            name.to_string(),
            ServiceEntry {
                name: name.to_string(),
                channel,
                is_system,
            },
        );
    }

    /// 添加子进程
    ///
    /// 当创建子进程时调用，注册其 bootstrap channel。
    pub fn add_child(&self, channel: Channel, privileged: bool) -> u64 {
        let id = {
            let mut next = self.next_conn_id.write();
            let id = *next;
            *next += 1;
            id
        };

        let port_key = id;

        // 绑定到 port
        let _ = self.port.bind(
            port_key,
            &channel,
            Signals::READABLE | Signals::PEER_CLOSED,
            BindOptions::Persistent,
        );

        self.children.write().insert(
            id,
            ChildConnection {
                id,
                channel,
                port_key,
                privileged,
            },
        );

        id
    }

    /// 移除子进程
    pub fn remove_child(&self, id: u64) {
        if let Some(child) = self.children.write().remove(&id) {
            let _ = self.port.unbind(child.port_key);
        }
    }

    /// 运行处理循环
    pub fn run(&self) -> radon_kernel::Result<()> {
        *self.running.write() = true;

        let mut packets = [PortPacket::zeroed(); 32];

        while *self.running.read() {
            let count = self.port.wait(&mut packets, Deadline::Infinite)?;

            for packet in &packets[..count] {
                self.handle_event(packet.key, packet.signals);
            }
        }

        Ok(())
    }

    /// 非阻塞处理（用于主循环中）
    pub fn poll(&self) -> radon_kernel::Result<()> {
        let mut packets = [PortPacket::zeroed(); 32];

        match self.port.try_wait(&mut packets) {
            Ok(count) => {
                for packet in &packets[..count] {
                    self.handle_event(packet.key, packet.signals);
                }
            }
            Err(_) => {}
        }

        Ok(())
    }

    /// 停止处理循环
    pub fn stop(&self) {
        *self.running.write() = false;
        let _ = self.port.queue_user(u64::MAX, [0; 4]);
    }

    /// 处理事件
    fn handle_event(&self, key: u64, signals: Signals) {
        let child_id = key;

        if signals.contains(Signals::PEER_CLOSED) {
            self.remove_child(child_id);
            return;
        }

        if signals.contains(Signals::READABLE) {
            self.handle_request(child_id);
        }
    }

    /// 处理请求
    fn handle_request(&self, child_id: u64) {
        let children = unsafe { self.children.as_mut_ptr().as_mut() }.unwrap();
        let child = match children.get(&child_id) {
            Some(c) => c,
            None => return,
        };

        let mut buf = [0u8; 256];
        let mut handles = [Handle::INVALID; 4];

        // 尝试接收请求
        let result = match child.channel.try_recv(&mut buf, &mut handles) {
            Ok(r) => r,
            Err(_) => return,
        };

        if result.data_len < BootstrapRequest::SIZE {
            return;
        }

        // 解析请求
        let request = match BootstrapRequest::from_bytes(&buf) {
            Some(r) => r,
            None => {
                self.send_error(&child.channel, ResponseStatus::InvalidRequest);
                return;
            }
        };

        // 获取服务名
        let name_start = BootstrapRequest::SIZE;
        let name_end = name_start + request.name_len as usize;

        if result.data_len < name_end {
            self.send_error(&child.channel, ResponseStatus::InvalidRequest);
            return;
        }

        let name = match core::str::from_utf8(&buf[name_start..name_end]) {
            Ok(s) => s,
            Err(_) => {
                self.send_error(&child.channel, ResponseStatus::InvalidRequest);
                return;
            }
        };

        let privileged = child.privileged;

        // 根据请求类型处理
        match request.request_type() {
            RequestType::GetService => {
                self.handle_get_service(&child_id, name);
            }
            RequestType::RegisterProvider => {
                if !privileged {
                    self.send_error_to(child_id, ResponseStatus::PermissionDenied);
                    return;
                }

                if result.handle_count > 0 && handles[0].is_valid() {
                    let channel = Channel::from_handle(OwnedHandle::from_raw(handles[0].raw()));
                    self.handle_register_provider(child_id, name, channel);
                } else {
                    self.send_error_to(child_id, ResponseStatus::InvalidRequest);
                }
            }
            RequestType::ListServices => {
                self.handle_list_services(child_id);
            }
            RequestType::Ping => {
                self.send_success_to(child_id);
            }
        }
    }

    /// 处理获取服务请求
    fn handle_get_service(&self, child_id: &u64, name: &str) {
        let services = self.services.read();

        let children = self.children.read();
        let child = match children.get(child_id) {
            Some(c) => c,
            None => return,
        };

        match services.get(name) {
            Some(entry) => {
                // 检查权限
                if entry.is_system && !child.privileged {
                    self.send_error(&child.channel, ResponseStatus::PermissionDenied);
                    return;
                }

                // 创建新的 Channel 对，将一端发送给请求者
                match Channel::create_pair() {
                    Ok((for_child, for_service)) => {
                        // 将 for_service 发送给服务
                        let _ = entry
                            .channel
                            .send_with_handles(&[0], &[for_service.handle()]);

                        // 发送响应
                        let response = BootstrapResponse::success().with_handle();
                        let _ = child
                            .channel
                            .send_with_handles(&response.to_bytes(), &[for_child.handle()]);
                    }
                    Err(_) => {
                        self.send_error(&child.channel, ResponseStatus::ServiceUnavailable);
                    }
                }
            }
            None => {
                self.send_error(&child.channel, ResponseStatus::NotFound);
            }
        }
    }

    /// 处理注册服务提供者请求
    fn handle_register_provider(&self, child_id: u64, name: &str, channel: Channel) {
        let mut services = self.services.write();

        if services.contains_key(name) {
            self.send_error_to(child_id, ResponseStatus::AlreadyExists);
            return;
        }

        services.insert(
            name.to_string(),
            ServiceEntry {
                name: name.to_string(),
                channel,
                is_system: false,
            },
        );

        self.send_success_to(child_id);
    }

    /// 处理列出服务请求
    fn handle_list_services(&self, child_id: u64) {
        let services = self.services.read();
        let children = self.children.read();

        let child = match children.get(&child_id) {
            Some(c) => c,
            None => return,
        };

        // 构造服务列表
        let mut data = Vec::new();
        let count = services.len() as u32;
        data.extend_from_slice(&count.to_le_bytes());

        for (name, _) in services.iter() {
            let name_bytes = name.as_bytes();
            data.extend_from_slice(&(name_bytes.len() as u32).to_le_bytes());
            data.extend_from_slice(name_bytes);
        }

        let mut response = BootstrapResponse::success();
        response.data_len = data.len() as u32;

        let mut resp_buf = Vec::with_capacity(BootstrapResponse::SIZE + data.len());
        resp_buf.extend_from_slice(&response.to_bytes());
        resp_buf.extend_from_slice(&data);

        let _ = child.channel.send(&resp_buf);
    }

    /// 发送错误响应
    fn send_error(&self, channel: &Channel, status: ResponseStatus) {
        let response = BootstrapResponse::error(status);
        let _ = channel.send(&response.to_bytes());
    }

    /// 发送错误响应（通过 child_id）
    fn send_error_to(&self, child_id: u64, status: ResponseStatus) {
        let children = unsafe { self.children.as_mut_ptr().as_mut() }.unwrap();
        if let Some(child) = children.get(&child_id) {
            self.send_error(&child.channel, status);
        }
    }

    /// 发送成功响应
    fn send_success_to(&self, child_id: u64) {
        let children = unsafe { self.children.as_mut_ptr().as_mut() }.unwrap();
        if let Some(child) = children.get(&child_id) {
            let response = BootstrapResponse::success();
            let _ = child.channel.send(&response.to_bytes());
        }
    }
}
