//! 服务注册表

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use libradon::syscall::clock_get;
use spin::RwLock;

use libradon::channel::Channel;

use crate::protocol::*;
use crate::{Error, Result};

/// 注册的服务
pub struct RegisteredService {
    /// 服务 ID
    pub id: u64,
    /// 服务名
    pub name: String,
    /// 描述
    pub description: String,
    /// 标志
    pub flags: ServiceFlags,
    /// 注册时间
    pub registered_at: u64,
    /// 所有者客户端 ID
    pub owner_id: u64,
    /// 服务 Channel（客户端连接时会克隆）
    pub channel: Channel,
    /// 连接计数
    pub connection_count: AtomicU64,
}

impl RegisteredService {
    pub fn to_info(&self) -> ServiceInfo {
        ServiceInfo {
            service_id: self.id,
            flags: self.flags.bits(),
            registered_at: self.registered_at,
            connection_count: self.connection_count.load(Ordering::Relaxed) as u32,
            name_len: self.name.len() as u32,
            desc_len: self.description.len() as u32,
            owner_pid: self.owner_id as u32,
        }
    }
}

/// 服务注册表
pub struct ServiceRegistry {
    /// 最大服务数
    max_services: usize,
    /// 下一个服务 ID
    next_id: AtomicU64,
    /// 按名称索引
    by_name: RwLock<BTreeMap<String, Arc<RegisteredService>>>,
    /// 按 ID 索引
    by_id: RwLock<BTreeMap<u64, Arc<RegisteredService>>>,
}

impl ServiceRegistry {
    pub fn new(max_services: usize) -> Self {
        Self {
            max_services,
            next_id: AtomicU64::new(1),
            by_name: RwLock::new(BTreeMap::new()),
            by_id: RwLock::new(BTreeMap::new()),
        }
    }

    /// 注册服务
    pub fn register(
        &self,
        name: String,
        description: String,
        flags: ServiceFlags,
        owner_id: u64,
        channel: Channel,
    ) -> Result<Arc<RegisteredService>> {
        // 检查服务数量限制
        if self.by_name.read().len() >= self.max_services {
            return Err(Error::ResourceExhausted);
        }

        // 检查名称是否已存在
        if self.by_name.read().contains_key(&name) {
            return Err(Error::AlreadyExists);
        }

        let id = self.next_id.fetch_add(1, Ordering::Relaxed);

        let service = Arc::new(RegisteredService {
            id,
            name: name.clone(),
            description,
            flags,
            registered_at: clock_get().unwrap(),
            owner_id,
            channel,
            connection_count: AtomicU64::new(0),
        });

        // 插入索引
        self.by_name.write().insert(name, service.clone());
        self.by_id.write().insert(id, service.clone());

        Ok(service)
    }

    /// 按名称查找
    pub fn lookup(&self, name: &str) -> Option<Arc<RegisteredService>> {
        self.by_name.read().get(name).cloned()
    }

    /// 按 ID 查找
    pub fn lookup_by_id(&self, id: u64) -> Option<Arc<RegisteredService>> {
        self.by_id.read().get(&id).cloned()
    }

    /// 按名称移除
    pub fn remove(&self, name: &str) -> Option<Arc<RegisteredService>> {
        let service = self.by_name.write().remove(name)?;
        self.by_id.write().remove(&service.id);
        Some(service)
    }

    /// 按 ID 移除
    pub fn remove_by_id(&self, id: u64) -> Option<Arc<RegisteredService>> {
        let service = self.by_id.write().remove(&id)?;
        self.by_name.write().remove(&service.name);
        Some(service)
    }

    /// 列出服务
    pub fn list(&self, prefix: &str, offset: usize, limit: usize) -> Vec<Arc<RegisteredService>> {
        let by_name = self.by_name.read();

        by_name
            .iter()
            .filter(|(name, service)| {
                name.starts_with(prefix) && !service.flags.contains(ServiceFlags::HIDDEN)
            })
            .skip(offset)
            .take(limit)
            .map(|(_, service)| service.clone())
            .collect()
    }

    /// 获取服务数量
    pub fn count(&self) -> usize {
        self.by_name.read().len()
    }

    /// 检查服务是否存在
    pub fn exists(&self, name: &str) -> bool {
        self.by_name.read().contains_key(name)
    }

    /// 移除所有者的所有服务
    pub fn remove_by_owner(&self, owner_id: u64) -> Vec<Arc<RegisteredService>> {
        let mut removed = Vec::new();

        let services: Vec<_> = self
            .by_id
            .read()
            .values()
            .filter(|s| s.owner_id == owner_id)
            .cloned()
            .collect();

        for service in services {
            if let Some(s) = self.remove(&service.name) {
                removed.push(s);
            }
        }

        removed
    }
}
