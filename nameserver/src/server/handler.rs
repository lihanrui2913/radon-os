//! 请求处理

use alloc::collections::BTreeMap;
use alloc::string::ToString;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use libradon::handle::OwnedHandle;
use spin::Mutex;

use libradon::{channel::Channel, handle::Handle};

use crate::protocol::*;
use crate::server::ClientConnection;
use crate::server::registry::ServiceRegistry;
use crate::server::watcher::WatcherManager;

/// 响应
pub struct Response {
    pub data: Vec<u8>,
    pub handles: Vec<Handle>,
}

impl Response {
    pub fn success(sequence: u32) -> Self {
        let header = MessageHeader::new_response(sequence, Status::Ok);
        Self {
            data: header.to_bytes().to_vec(),
            handles: Vec::new(),
        }
    }

    pub fn error(sequence: u32, status: Status) -> Self {
        let header = MessageHeader::new_response(sequence, status);
        Self {
            data: header.to_bytes().to_vec(),
            handles: Vec::new(),
        }
    }

    pub fn with_data(mut self, data: &[u8]) -> Self {
        // 更新头部的 data_len
        if self.data.len() >= MessageHeader::SIZE {
            let len = data.len() as u32;
            self.data[24..28].copy_from_slice(&len.to_le_bytes());
        }
        self.data.extend_from_slice(data);
        self
    }

    pub fn with_handles(mut self, handles: Vec<Handle>) -> Self {
        // 更新头部的 handle_count
        if self.data.len() >= MessageHeader::SIZE {
            let count = handles.len() as u32;
            self.data[28..32].copy_from_slice(&count.to_le_bytes());
        }
        self.handles = handles;
        self
    }
}

/// 请求处理器
pub struct RequestHandler {
    registry: Arc<ServiceRegistry>,
    watchers: Arc<WatcherManager>,
}

impl RequestHandler {
    pub fn new(registry: Arc<ServiceRegistry>, watchers: Arc<WatcherManager>) -> Self {
        Self { registry, watchers }
    }

    /// 处理请求
    pub fn handle(
        &self,
        client_id: u64,
        header: &MessageHeader,
        data: &[u8],
        handles: &[Handle],
        clients: &Mutex<BTreeMap<u64, ClientConnection>>,
    ) -> Response {
        let sequence = header.sequence;

        match header.opcode() {
            OpCode::Register => self.handle_register(client_id, sequence, data, handles, clients),
            OpCode::Unregister => self.handle_unregister(client_id, sequence, data, clients),
            OpCode::Lookup => self.handle_lookup(sequence, data),
            OpCode::Connect => self.handle_connect(sequence, data),
            OpCode::List => self.handle_list(sequence, data),
            OpCode::Exists => self.handle_exists(sequence, data),
            OpCode::Watch => self.handle_watch(client_id, sequence, data, clients),
            OpCode::Unwatch => self.handle_unwatch(client_id, sequence, data, clients),
            OpCode::GetInfo => self.handle_get_info(sequence, data),
            _ => Response::error(sequence, Status::InvalidArgument),
        }
    }

    /// 处理注册请求
    fn handle_register(
        &self,
        client_id: u64,
        sequence: u32,
        data: &[u8],
        handles: &[Handle],
        clients: &Mutex<BTreeMap<u64, ClientConnection>>,
    ) -> Response {
        // 检查句柄
        if handles.is_empty() {
            return Response::error(sequence, Status::InvalidArgument);
        }

        // 解析请求
        if data.len() < core::mem::size_of::<RegisterRequest>() {
            return Response::error(sequence, Status::InvalidArgument);
        }

        let req: RegisterRequest =
            unsafe { (data.as_ptr() as *const RegisterRequest).read_unaligned() };

        let name_start = core::mem::size_of::<RegisterRequest>();
        let name_end = name_start + req.name_len as usize;
        let desc_end = name_end + req.desc_len as usize;

        if data.len() < desc_end {
            return Response::error(sequence, Status::InvalidArgument);
        }

        let name = match core::str::from_utf8(&data[name_start..name_end]) {
            Ok(s) => s.to_string(),
            Err(_) => return Response::error(sequence, Status::InvalidArgument),
        };

        let description = match core::str::from_utf8(&data[name_end..desc_end]) {
            Ok(s) => s.to_string(),
            Err(_) => return Response::error(sequence, Status::InvalidArgument),
        };

        if name.len() > MAX_SERVICE_NAME_LEN {
            return Response::error(sequence, Status::NameTooLong);
        }

        // 创建 Channel
        let channel = Channel::from_handle(OwnedHandle::from_raw(handles[0].raw()));

        // 注册服务
        let flags = ServiceFlags::from_bits_truncate(req.flags);
        match self
            .registry
            .register(name.clone(), description, flags, client_id, channel)
        {
            Ok(service) => {
                let mut clients_guard = clients.lock();
                // 记录到客户端
                if let Some(client) = clients_guard.get_mut(&client_id) {
                    client.registered_services.push(service.id);
                }
                drop(clients_guard);

                // 通知监视者
                self.watchers.notify_online(&name, service.id, clients);

                // 构造响应
                let resp = RegisterResponse {
                    service_id: service.id,
                };

                let resp_bytes = unsafe {
                    core::slice::from_raw_parts(
                        &resp as *const _ as *const u8,
                        core::mem::size_of::<RegisterResponse>(),
                    )
                };

                Response::success(sequence).with_data(resp_bytes)
            }
            Err(crate::Error::AlreadyExists) => Response::error(sequence, Status::AlreadyExists),
            Err(crate::Error::ResourceExhausted) => {
                Response::error(sequence, Status::ResourceExhausted)
            }
            Err(_) => Response::error(sequence, Status::InternalError),
        }
    }

    /// 处理注销请求
    fn handle_unregister(
        &self,
        client_id: u64,
        sequence: u32,
        data: &[u8],
        clients: &Mutex<BTreeMap<u64, ClientConnection>>,
    ) -> Response {
        if data.len() < 4 {
            return Response::error(sequence, Status::InvalidArgument);
        }

        let name_len = u32::from_le_bytes(data[..4].try_into().unwrap()) as usize;

        if data.len() < 4 + name_len {
            return Response::error(sequence, Status::InvalidArgument);
        }

        let name = match core::str::from_utf8(&data[4..4 + name_len]) {
            Ok(s) => s,
            Err(_) => return Response::error(sequence, Status::InvalidArgument),
        };

        // 查找服务
        let service = match self.registry.lookup(name) {
            Some(s) => s,
            None => return Response::error(sequence, Status::NotFound),
        };

        // 检查所有权
        if service.owner_id != client_id {
            return Response::error(sequence, Status::PermissionDenied);
        }

        // 移除服务
        let service = self.registry.remove(name).unwrap();

        // 从客户端记录中移除
        if let Some(client) = clients.lock().get_mut(&client_id) {
            client.registered_services.retain(|id| *id != service.id);
        }

        // 通知监视者
        self.watchers
            .notify_offline(&service.name, service.id, clients);

        Response::success(sequence)
    }

    /// 处理查找请求
    fn handle_lookup(&self, sequence: u32, data: &[u8]) -> Response {
        if data.len() < core::mem::size_of::<LookupRequest>() {
            return Response::error(sequence, Status::InvalidArgument);
        }

        let req: LookupRequest =
            unsafe { (data.as_ptr() as *const LookupRequest).read_unaligned() };

        let name_start = core::mem::size_of::<LookupRequest>();
        let name_end = name_start + req.name_len as usize;

        if data.len() < name_end {
            return Response::error(sequence, Status::InvalidArgument);
        }

        let name = match core::str::from_utf8(&data[name_start..name_end]) {
            Ok(s) => s,
            Err(_) => return Response::error(sequence, Status::InvalidArgument),
        };

        // 查找服务
        match self.registry.lookup(name) {
            Some(service) => {
                let info = service.to_info();
                let info_bytes = unsafe {
                    core::slice::from_raw_parts(
                        &info as *const _ as *const u8,
                        core::mem::size_of::<ServiceInfo>(),
                    )
                };

                Response::success(sequence).with_data(info_bytes)
            }
            None => Response::error(sequence, Status::NotFound),
        }
    }

    /// 处理连接请求
    fn handle_connect(&self, sequence: u32, data: &[u8]) -> Response {
        if data.len() < core::mem::size_of::<LookupRequest>() {
            return Response::error(sequence, Status::InvalidArgument);
        }

        let req: LookupRequest =
            unsafe { (data.as_ptr() as *const LookupRequest).read_unaligned() };

        let name_start = core::mem::size_of::<LookupRequest>();
        let name_end = name_start + req.name_len as usize;

        if data.len() < name_end {
            return Response::error(sequence, Status::InvalidArgument);
        }

        let name = match core::str::from_utf8(&data[name_start..name_end]) {
            Ok(s) => s,
            Err(_) => return Response::error(sequence, Status::InvalidArgument),
        };

        // 查找服务
        match self.registry.lookup(name) {
            Some(service) => {
                // 创建与服务通信的新 Channel
                let (mut client_end, server_end) = match Channel::create_pair() {
                    Ok(pair) => pair,
                    Err(_) => return Response::error(sequence, Status::InternalError),
                };

                // 将 server_end 发送给服务
                match service
                    .channel
                    .send_with_handles(&[0], &[server_end.handle()])
                {
                    Ok(_) => {}
                    Err(_) => return Response::error(sequence, Status::InternalError),
                };

                service
                    .connection_count
                    .fetch_add(1, core::sync::atomic::Ordering::Relaxed);

                // 返回 client_end 给调用者
                // 此处必须设置 nodrop 否则会传输失败
                client_end.with_nodrop(true);
                Response::success(sequence).with_handles(vec![client_end.handle()])
            }
            None => Response::error(sequence, Status::NotFound),
        }
    }

    /// 处理列表请求
    fn handle_list(&self, sequence: u32, data: &[u8]) -> Response {
        if data.len() < core::mem::size_of::<ListRequest>() {
            return Response::error(sequence, Status::InvalidArgument);
        }

        let req: ListRequest = unsafe { (data.as_ptr() as *const ListRequest).read_unaligned() };

        let contain_name_len_start = core::mem::size_of::<ListRequest>();
        let contain_name_len_end = contain_name_len_start + req.contain_name_len as usize;

        let contain_name = if req.contain_name_len > 0 && data.len() >= contain_name_len_end {
            core::str::from_utf8(&data[contain_name_len_start..contain_name_len_end]).unwrap_or("")
        } else {
            ""
        };

        let services = self.registry.list(contain_name, req.limit as usize);
        let total = self.registry.count();

        // 构造响应
        let resp = ListResponse {
            total_count: total as u32,
            returned_count: services.len() as u32,
        };

        let mut resp_data = Vec::new();
        resp_data.extend_from_slice(unsafe {
            core::slice::from_raw_parts(
                &resp as *const _ as *const u8,
                core::mem::size_of::<ListResponse>(),
            )
        });

        for service in &services {
            let info = service.to_info();
            resp_data.extend_from_slice(unsafe {
                core::slice::from_raw_parts(
                    &info as *const _ as *const u8,
                    core::mem::size_of::<ServiceInfo>(),
                )
            });
            resp_data.extend_from_slice(service.name.as_bytes());
            resp_data.extend_from_slice(service.description.as_bytes());
        }

        Response::success(sequence).with_data(&resp_data)
    }

    /// 处理存在检查请求
    fn handle_exists(&self, sequence: u32, data: &[u8]) -> Response {
        if data.len() < 4 {
            return Response::error(sequence, Status::InvalidArgument);
        }

        let name_len = u32::from_le_bytes(data[..4].try_into().unwrap()) as usize;

        if data.len() < 4 + name_len {
            return Response::error(sequence, Status::InvalidArgument);
        }

        let name = match core::str::from_utf8(&data[4..4 + name_len]) {
            Ok(s) => s,
            Err(_) => return Response::error(sequence, Status::InvalidArgument),
        };

        if self.registry.exists(name) {
            Response::success(sequence)
        } else {
            Response::error(sequence, Status::NotFound)
        }
    }

    /// 处理监视请求
    fn handle_watch(
        &self,
        client_id: u64,
        sequence: u32,
        data: &[u8],
        clients: &Mutex<BTreeMap<u64, ClientConnection>>,
    ) -> Response {
        if data.len() < core::mem::size_of::<WatchRequest>() {
            return Response::error(sequence, Status::InvalidArgument);
        }

        let req: WatchRequest = unsafe { (data.as_ptr() as *const WatchRequest).read_unaligned() };

        let pattern = if req.name_len > 0 {
            let name_start = core::mem::size_of::<WatchRequest>();
            let name_end = name_start + req.name_len as usize;

            if data.len() < name_end {
                return Response::error(sequence, Status::InvalidArgument);
            }

            Some(
                core::str::from_utf8(&data[name_start..name_end])
                    .unwrap_or("")
                    .to_string(),
            )
        } else {
            None
        };

        let events = WatchEvents::from_bits_truncate(req.events);
        let watch_id = self.watchers.add(client_id, pattern, events);

        // 记录到客户端
        if let Some(client) = clients.lock().get_mut(&client_id) {
            client.watches.push(watch_id);
        }

        Response::success(sequence)
    }

    /// 处理取消监视请求
    fn handle_unwatch(
        &self,
        client_id: u64,
        sequence: u32,
        data: &[u8],
        clients: &Mutex<BTreeMap<u64, ClientConnection>>,
    ) -> Response {
        if data.len() < 4 {
            return Response::error(sequence, Status::InvalidArgument);
        }

        let watch_id = u32::from_le_bytes(data[..4].try_into().unwrap());

        self.watchers.remove(watch_id);

        // 从客户端记录中移除
        if let Some(client) = clients.lock().get_mut(&client_id) {
            client.watches.retain(|id| *id != watch_id);
        }

        Response::success(sequence)
    }

    /// 处理获取信息请求
    fn handle_get_info(&self, sequence: u32, data: &[u8]) -> Response {
        if data.len() < 4 {
            return Response::error(sequence, Status::InvalidArgument);
        }

        let name_len = u32::from_le_bytes(data[..4].try_into().unwrap()) as usize;

        if data.len() < 4 + name_len {
            return Response::error(sequence, Status::InvalidArgument);
        }

        let name = match core::str::from_utf8(&data[4..4 + name_len]) {
            Ok(s) => s,
            Err(_) => return Response::error(sequence, Status::InvalidArgument),
        };

        match self.registry.lookup(name) {
            Some(service) => {
                let info = service.to_info();

                let mut resp_data = Vec::new();
                resp_data.extend_from_slice(unsafe {
                    core::slice::from_raw_parts(
                        &info as *const _ as *const u8,
                        core::mem::size_of::<ServiceInfo>(),
                    )
                });
                resp_data.extend_from_slice(service.name.as_bytes());
                resp_data.extend_from_slice(service.description.as_bytes());

                Response::success(sequence).with_data(&resp_data)
            }
            None => Response::error(sequence, Status::NotFound),
        }
    }
}
