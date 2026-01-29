//! 驱动服务端框架

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use alloc::{boxed::Box, format};
use libradon::info;
use radon_kernel::Error;
use spin::Mutex;

use libradon::{
    channel::Channel,
    handle::Handle,
    port::{BindOptions, Deadline, Port, PortPacket},
    signal::Signals,
};

use crate::protocol::{DriverOp, MessageHeader, Request, Response};
use crate::{DriverError, Result};

/// 请求处理器 trait
pub trait RequestHandler: Send + Sync {
    /// 处理请求
    fn handle(&self, request: &Request, ctx: &RequestContext) -> Response;

    /// 处理连接建立
    fn on_connect(&self, _ctx: &ConnectionContext) -> Result<()> {
        Ok(())
    }

    /// 处理连接断开
    fn on_disconnect(&self, _ctx: &ConnectionContext) {}
}

/// 请求上下文
pub struct RequestContext {
    /// 连接 ID
    pub conn_id: u64,
    /// 请求 ID
    pub request_id: u32,
}

/// 连接上下文
pub struct ConnectionContext {
    /// 连接 ID
    pub conn_id: u64,
    /// 客户端信息
    pub client_info: Option<String>,
}

/// 客户端连接
struct ClientConnection {
    id: u64,
    channel: Channel,
    key: u64,
}

/// 驱动服务器
pub struct DriverServer {
    /// 服务名称
    name: String,
    /// 接受连接的 Channel
    accept_channel: Channel,
    /// 事件 Port
    port: Port,
    /// 客户端连接
    clients: Mutex<BTreeMap<u64, ClientConnection>>,
    /// 下一个连接 ID
    next_conn_id: Mutex<u64>,
    /// 请求处理器
    handler: Arc<dyn RequestHandler>,
    /// 是否运行中
    running: Mutex<bool>,
}

impl DriverServer {
    /// 创建新的驱动服务器
    pub fn new(name: &str, handler: Arc<dyn RequestHandler>) -> Result<Self> {
        let (accept_server, accept_client) = Channel::create_pair()?;
        let port = Port::create()?;

        // 绑定接受 channel 到 port
        port.bind(
            0, // key = 0 用于接受连接
            &accept_server,
            Signals::READABLE | Signals::PEER_CLOSED,
            BindOptions::Persistent,
        )?;

        // 注册到命名服务
        nameserver::client::register(&format!("driver.{}", name), &accept_client)
            .map_err(|e| Error::from(e))?;

        info!("Driver {} registered.", name);

        Ok(Self {
            name: name.into(),
            accept_channel: accept_server,
            port,
            clients: Mutex::new(BTreeMap::new()),
            next_conn_id: Mutex::new(1),
            handler,
            running: Mutex::new(false),
        })
    }

    /// 获取服务名称
    pub fn name(&self) -> &str {
        &self.name
    }

    /// 运行服务器
    pub fn run(&self) -> Result<()> {
        *self.running.lock() = true;

        let mut packets = [PortPacket::zeroed(); 32];

        while *self.running.lock() {
            let count = self.port.wait(&mut packets, Deadline::Infinite)?;

            for i in 0..count {
                let packet = &packets[i];

                if packet.key == 0 {
                    // 接受连接请求
                    self.handle_accept()?;
                } else {
                    // 客户端消息
                    self.handle_client_event(packet.key, packet.signals)?;
                }
            }
        }

        Ok(())
    }

    /// 停止服务器
    pub fn stop(&self) {
        *self.running.lock() = false;
        // 发送一个唤醒事件
        let _ = self.port.queue_user(u64::MAX, [0; 4]);
    }

    /// 处理连接请求
    fn handle_accept(&self) -> Result<()> {
        let mut buf = [0u8; 256];
        let mut handles = [Handle::INVALID; 4];

        loop {
            match self
                .accept_channel
                .try_recv_with_handles(&mut buf, &mut handles)
            {
                Ok(result) if result.handle_count > 0 => {
                    let client_channel = Channel::from_handle(
                        libradon::handle::OwnedHandle::from_raw(handles[0].raw()),
                    );

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
        let conn_id = {
            let mut next = self.next_conn_id.lock();
            let id = *next;
            *next += 1;
            id
        };

        // 绑定到 port
        let key = conn_id;
        self.port.bind(
            key,
            &channel,
            Signals::READABLE | Signals::PEER_CLOSED,
            BindOptions::Persistent,
        )?;

        // 调用连接回调
        let ctx = ConnectionContext {
            conn_id,
            client_info: None,
        };

        if let Err(e) = self.handler.on_connect(&ctx) {
            // 连接被拒绝
            return Err(e);
        }

        // 保存连接
        self.clients.lock().insert(
            conn_id,
            ClientConnection {
                id: conn_id,
                channel,
                key,
            },
        );

        Ok(conn_id)
    }

    /// 移除客户端
    fn remove_client(&self, conn_id: u64) {
        if let Some(client) = self.clients.lock().remove(&conn_id) {
            let _ = self.port.unbind(client.key);

            let ctx = ConnectionContext {
                conn_id,
                client_info: None,
            };
            self.handler.on_disconnect(&ctx);
        }
    }

    /// 处理客户端事件
    fn handle_client_event(&self, key: u64, signals: Signals) -> Result<()> {
        let conn_id = key;

        if signals.contains(Signals::PEER_CLOSED) {
            self.remove_client(conn_id);
            return Ok(());
        }

        if signals.contains(Signals::READABLE) {
            self.handle_client_request(conn_id)?;
        }

        Ok(())
    }

    /// 处理客户端请求
    fn handle_client_request(&self, conn_id: u64) -> Result<()> {
        let clients = self.clients.lock();
        let client = match clients.get(&conn_id) {
            Some(c) => c,
            None => return Ok(()),
        };

        let mut buf = [0u8; 4096];
        let mut handles = [Handle::INVALID; 16];

        loop {
            match client.channel.try_recv_with_handles(&mut buf, &mut handles) {
                Ok(result) if result.data_len >= MessageHeader::SIZE => {
                    // 解析请求
                    let header = MessageHeader::from_bytes(&buf[..MessageHeader::SIZE])
                        .ok_or(DriverError::InvalidArgument)?;

                    let data = buf
                        [MessageHeader::SIZE..MessageHeader::SIZE + header.data_len as usize]
                        .to_vec();
                    let req_handles = handles[..result.handle_count].iter().map(|h| *h).collect();

                    let request = Request {
                        header,
                        data,
                        handles: req_handles,
                    };

                    // 处理请求
                    let ctx = RequestContext {
                        conn_id,
                        request_id: header.request_id,
                    };

                    let response = self.handler.handle(&request, &ctx);

                    // 发送响应（如果需要）
                    if header
                        .flags
                        .contains(crate::protocol::MessageFlags::NEED_REPLY)
                    {
                        let resp_data = response.encode();
                        let resp_handles: Vec<_> = response.handles.iter().map(|h| *h).collect();

                        client
                            .channel
                            .send_with_handles(&resp_data, &resp_handles)?;
                    }
                }
                Ok(_) => break,
                Err(e) if e.errno == radon_kernel::EAGAIN => break,
                Err(_) => {
                    drop(clients);
                    self.remove_client(conn_id);
                    return Ok(());
                }
            }
        }

        Ok(())
    }
}

/// 服务构建器
pub struct ServiceBuilder {
    name: String,
}

impl ServiceBuilder {
    pub fn new(name: &str) -> Self {
        Self { name: name.into() }
    }

    pub fn build<H: RequestHandler + 'static>(self, handler: H) -> Result<DriverServer> {
        DriverServer::new(&self.name, Arc::new(handler))
    }
}

type HandlerFn = Box<dyn Fn(&Request, &RequestContext) -> Response + Send + Sync>;

/// 函数式请求处理器
pub struct FnHandler {
    handlers: BTreeMap<u32, HandlerFn>,
    default_handler: Option<HandlerFn>,
}

impl FnHandler {
    pub fn new() -> Self {
        Self {
            handlers: BTreeMap::new(),
            default_handler: None,
        }
    }

    /// 注册处理函数
    pub fn on<F>(mut self, op: DriverOp, handler: F) -> Self
    where
        F: Fn(&Request, &RequestContext) -> Response + Send + Sync + 'static,
    {
        self.handlers.insert(op as u32, Box::new(handler));
        self
    }

    /// 注册默认处理函数
    pub fn default<F>(mut self, handler: F) -> Self
    where
        F: Fn(&Request, &RequestContext) -> Response + Send + Sync + 'static,
    {
        self.default_handler = Some(Box::new(handler));
        self
    }
}

impl RequestHandler for FnHandler {
    fn handle(&self, request: &Request, ctx: &RequestContext) -> Response {
        if let Some(handler) = self.handlers.get(&request.header.op) {
            handler(request, ctx)
        } else if let Some(ref default) = self.default_handler {
            default(request, ctx)
        } else {
            Response::error(ctx.request_id, -1)
        }
    }
}
