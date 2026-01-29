//! 服务监视管理

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};
use spin::{Mutex, RwLock};

use crate::protocol::*;
use crate::server::ClientConnection;

/// 监视器
struct Watcher {
    /// 监视 ID
    id: u32,
    /// 所有者客户端 ID
    client_id: u64,
    /// 监视模式（服务名前缀，None 表示监视所有）
    pattern: Option<String>,
    /// 监视的事件类型
    events: WatchEvents,
}

impl Watcher {
    fn matches(&self, name: &str, event: WatchEvents) -> bool {
        if !self.events.contains(event) {
            return false;
        }

        match &self.pattern {
            Some(pattern) => name.starts_with(pattern),
            None => true,
        }
    }
}

/// 监视器管理器
pub struct WatcherManager {
    /// 最大监视器数
    max_watchers: usize,
    /// 下一个监视 ID
    next_id: AtomicU32,
    /// 监视器列表
    watchers: RwLock<BTreeMap<u32, Watcher>>,
}

impl WatcherManager {
    pub fn new(max_watchers: usize) -> Self {
        Self {
            max_watchers,
            next_id: AtomicU32::new(1),
            watchers: RwLock::new(BTreeMap::new()),
        }
    }

    /// 添加监视器
    pub fn add(&self, client_id: u64, pattern: Option<String>, events: WatchEvents) -> u32 {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);

        let watcher = Watcher {
            id,
            client_id,
            pattern,
            events,
        };

        self.watchers.write().insert(id, watcher);

        id
    }

    /// 移除监视器
    pub fn remove(&self, id: u32) {
        self.watchers.write().remove(&id);
    }

    /// 移除客户端的所有监视器
    pub fn remove_by_client(&self, client_id: u64) {
        self.watchers
            .write()
            .retain(|_, w| w.client_id != client_id);
    }

    /// 通知服务上线
    pub fn notify_online(
        &self,
        name: &str,
        service_id: u64,
        clients: &Mutex<BTreeMap<u64, ClientConnection>>,
    ) {
        self.notify(
            name,
            service_id,
            WatchEvents::ONLINE,
            OpCode::NotifyOnline,
            clients,
        );
    }

    /// 通知服务下线
    pub fn notify_offline(
        &self,
        name: &str,
        service_id: u64,
        clients: &Mutex<BTreeMap<u64, ClientConnection>>,
    ) {
        self.notify(
            name,
            service_id,
            WatchEvents::OFFLINE,
            OpCode::NotifyOffline,
            clients,
        );
    }

    /// 发送通知
    fn notify(
        &self,
        name: &str,
        service_id: u64,
        event: WatchEvents,
        opcode: OpCode,
        clients: &Mutex<BTreeMap<u64, ClientConnection>>,
    ) {
        let watchers = self.watchers.read();
        let clients_guard = clients.lock();

        // 收集需要通知的客户端
        let to_notify: Vec<_> = watchers
            .values()
            .filter(|w| w.matches(name, event))
            .filter_map(|w| clients_guard.get(&w.client_id))
            .collect();

        if to_notify.is_empty() {
            return;
        }

        // 构造通知消息
        let notif_data = NotificationData {
            service_id,
            name_len: name.len() as u32,
            reserved: 0,
        };

        let mut header = MessageHeader::new_notification(opcode);
        header.data_len = (core::mem::size_of::<NotificationData>() + name.len()) as u32;

        let mut msg = Vec::with_capacity(MessageHeader::SIZE + header.data_len as usize);
        msg.extend_from_slice(&header.to_bytes());
        msg.extend_from_slice(unsafe {
            core::slice::from_raw_parts(
                &notif_data as *const _ as *const u8,
                core::mem::size_of::<NotificationData>(),
            )
        });
        msg.extend_from_slice(name.as_bytes());

        // 发送通知
        for client in to_notify {
            let _ = client.channel.send(&msg);
        }
    }

    /// 获取监视器数量
    pub fn count(&self) -> usize {
        self.watchers.read().len()
    }
}
