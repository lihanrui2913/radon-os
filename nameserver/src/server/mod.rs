//! Name Server 服务端实现

pub mod handler;
pub mod registry;
pub mod watcher;

use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::Mutex;

use libradon::handle::OwnedHandle;
use libradon::port::{BindOptions, Deadline};
use libradon::{channel::Channel, handle::Handle, port::Port, port::PortPacket, signal::Signals};

use crate::protocol::*;
use crate::{Error, Result};
use handler::RequestHandler;
use registry::ServiceRegistry;
use watcher::WatcherManager;

/// Name Server 配置
pub struct Config {
    /// 最大服务数
    pub max_services: usize,
    /// 最大客户端连接数
    pub max_clients: usize,
    /// 最大监视器数
    pub max_watchers: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            max_services: 1024,
            max_clients: 256,
            max_watchers: 512,
        }
    }
}

/// 客户端连接
struct ClientConnection {
    id: u64,
    channel: Channel,
    key: u64,
    /// 该客户端注册的服务
    registered_services: Vec<u64>,
    /// 该客户端的监视
    watches: Vec<u32>,
}

/// Name Server
pub struct NameServer {
    /// 配置
    config: Config,
    /// 服务注册表
    registry: Arc<ServiceRegistry>,
    /// 监视器管理器
    watchers: Arc<WatcherManager>,
    /// 事件 Port
    port: Port,
    /// 接受连接的 Channel
    accept_channel: Channel,
    /// 客户端连接
    clients: Mutex<BTreeMap<u64, ClientConnection>>,
    /// 下一个客户端 ID
    next_client_id: Mutex<u64>,
    /// 是否运行中
    running: Mutex<bool>,
}

impl NameServer {
    /// 创建 Name Server
    pub fn new(config: Config) -> Result<(Self, Channel)> {
        let (accept_server, accept_client) = Channel::create_pair()?;
        let port = Port::create()?;

        // 绑定接受 channel
        port.bind(
            0, // key = 0 用于接受连接
            &accept_server,
            Signals::READABLE | Signals::PEER_CLOSED,
            BindOptions::Persistent,
        )?;

        let registry = Arc::new(ServiceRegistry::new(config.max_services));
        let watchers = Arc::new(WatcherManager::new(config.max_watchers));

        Ok((
            Self {
                config,
                registry,
                watchers,
                port,
                accept_channel: accept_server,
                clients: Mutex::new(BTreeMap::new()),
                next_client_id: Mutex::new(1),
                running: Mutex::new(false),
            },
            accept_client,
        ))
    }

    /// 运行 Name Server
    pub fn run(&self) -> Result<()> {
        *self.running.lock() = true;

        let mut packets = [PortPacket::zeroed(); 32];

        while *self.running.lock() {
            let count = self.port.wait(&mut packets, Deadline::Infinite)?;

            for i in 0..count {
                let packet = &packets[i];

                if packet.key == 0 {
                    // 新连接
                    self.handle_accept()?;
                } else {
                    // 客户端事件
                    self.handle_client_event(packet.key, packet.signals)?;
                }
            }
        }

        Ok(())
    }

    /// 停止 Name Server
    pub fn stop(&self) {
        *self.running.lock() = false;
        let _ = self.port.queue_user(u64::MAX, [0; 4]);
    }

    /// 处理新连接
    fn handle_accept(&self) -> Result<()> {
        let mut buf = [0u8; 256];
        let mut handles = [Handle::INVALID; 4];

        loop {
            match self.accept_channel.try_recv(&mut buf, &mut handles) {
                Ok(result) if result.handle_count > 0 => {
                    let client_channel =
                        Channel::from_handle(OwnedHandle::from_raw(handles[0].raw()));
                    self.add_client(client_channel)?;
                }
                Ok(_) => break,
                Err(e) if e.errno == radon_kernel::EAGAIN => break,
                Err(e) => return Err(e.into()),
            }
        }

        Ok(())
    }

    /// 添加客户端
    fn add_client(&self, channel: Channel) -> Result<u64> {
        let client_id = {
            let mut next = self.next_client_id.lock();
            let id = *next;
            *next += 1;
            id
        };

        // 检查客户端数量限制
        if self.clients.lock().len() >= self.config.max_clients {
            return Err(Error::ResourceExhausted);
        }

        // 绑定到 port
        let key = client_id;
        self.port.bind(
            key,
            &channel,
            Signals::READABLE | Signals::PEER_CLOSED,
            BindOptions::Persistent,
        )?;

        // 保存连接
        self.clients.lock().insert(
            client_id,
            ClientConnection {
                id: client_id,
                channel,
                key,
                registered_services: Vec::new(),
                watches: Vec::new(),
            },
        );

        Ok(client_id)
    }

    /// 移除客户端
    fn remove_client(&self, client_id: u64) {
        if let Some(client) = self.clients.lock().remove(&client_id) {
            // 解绑 port
            let _ = self.port.unbind(client.key);

            // 注销该客户端注册的所有服务
            for service_id in &client.registered_services {
                if let Some(service) = self.registry.remove_by_id(*service_id) {
                    // 通知监视者
                    self.watchers
                        .notify_offline(&service.name, *service_id, &self.clients);
                }
            }

            // 移除监视
            for watch_id in &client.watches {
                self.watchers.remove(*watch_id);
            }
        }
    }

    /// 处理客户端事件
    fn handle_client_event(&self, key: u64, signals: Signals) -> Result<()> {
        let client_id = key;

        if signals.contains(Signals::PEER_CLOSED) {
            self.remove_client(client_id);
            return Ok(());
        }

        if signals.contains(Signals::READABLE) {
            self.handle_client_request(client_id)?;
        }

        Ok(())
    }

    /// 处理客户端请求
    fn handle_client_request(&self, client_id: u64) -> Result<()> {
        let clients = self.clients.lock();
        let client = match clients.get(&client_id) {
            Some(c) => c,
            None => return Ok(()),
        };

        let mut buf = [0u8; 4096];
        let mut handles = [Handle::INVALID; 16];

        loop {
            match client.channel.try_recv(&mut buf, &mut handles) {
                Ok(result) if result.data_len >= MessageHeader::SIZE => {
                    let header = match MessageHeader::from_bytes(&buf) {
                        Some(h) => h,
                        None => continue,
                    };

                    let data =
                        &buf[MessageHeader::SIZE..MessageHeader::SIZE + header.data_len as usize];
                    let req_handles = &handles[..result.handle_count];

                    // 创建请求处理器
                    let handler = RequestHandler::new(self.registry.clone(), self.watchers.clone());

                    drop(clients);

                    // 处理请求
                    let response =
                        handler.handle(client_id, &header, data, req_handles, &self.clients);

                    // 发送响应
                    let clients = self.clients.lock();
                    if let Some(client) = clients.get(&client_id) {
                        let _ = client
                            .channel
                            .send_with_handles(&response.data, &response.handles);
                    }

                    return Ok(());
                }
                Ok(_) => break,
                Err(e) if e.errno == radon_kernel::EAGAIN => break,
                Err(_) => {
                    drop(clients);
                    self.remove_client(client_id);
                    return Ok(());
                }
            }
        }

        Ok(())
    }
}
