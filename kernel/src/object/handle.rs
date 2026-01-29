use super::KernelObject;
use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;
use bitflags::bitflags;

/// 用户空间句柄
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Handle(pub u32);

impl Handle {
    pub const INVALID: Handle = Handle(0);

    #[inline]
    pub const fn from_raw(raw: u32) -> Self {
        Handle(raw)
    }

    #[inline]
    pub const fn raw(&self) -> u32 {
        self.0
    }

    #[inline]
    pub const fn is_valid(&self) -> bool {
        self.0 != 0
    }
}

impl From<usize> for Handle {
    fn from(v: usize) -> Self {
        Handle(v as u32)
    }
}

impl From<Handle> for usize {
    fn from(h: Handle) -> Self {
        h.0 as usize
    }
}

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Rights: u32 {
        const READ      = 1 << 0;
        const WRITE     = 1 << 1;
        const EXECUTE   = 1 << 2;
        const MAP       = 1 << 3;
        const DUPLICATE = 1 << 4;
        const TRANSFER  = 1 << 5;
        const WAIT      = 1 << 6;
        const SIGNAL    = 1 << 7;
        const MANAGE    = 1 << 8;

        const BASIC = Self::READ.bits() | Self::WRITE.bits() | Self::WAIT.bits();
        const ALL = u32::MAX;
    }
}

/// 句柄表项
#[derive(Clone)]
pub struct HandleEntry {
    pub object: Arc<dyn KernelObject>,
    pub rights: Rights,
}

/// 进程句柄表
pub struct HandleTable {
    handles: BTreeMap<Handle, HandleEntry>,
    next_id: u32,
}

impl HandleTable {
    pub fn new() -> Self {
        Self {
            handles: BTreeMap::new(),
            next_id: 1, // 0 是无效句柄
        }
    }

    /// 插入对象，返回句柄
    pub fn insert(&mut self, object: Arc<dyn KernelObject>, rights: Rights) -> Handle {
        let handle = Handle(self.next_id);
        self.next_id += 1;
        self.handles.insert(handle, HandleEntry { object, rights });
        handle
    }

    /// 获取对象（检查权限）
    pub fn get(&self, handle: Handle, required: Rights) -> Option<Arc<dyn KernelObject>> {
        let entry = self.handles.get(&handle)?;
        if entry.rights.contains(required) {
            Some(entry.object.clone())
        } else {
            None
        }
    }

    /// 获取对象和权限（不检查权限）
    pub fn get_entry(&self, handle: Handle) -> Option<&HandleEntry> {
        self.handles.get(&handle)
    }

    /// 获取对象（不检查权限）
    pub fn get_unchecked(&self, handle: Handle) -> Option<Arc<dyn KernelObject>> {
        self.handles.get(&handle).map(|e| e.object.clone())
    }

    /// 获取权限
    pub fn get_rights(&self, handle: Handle) -> Option<Rights> {
        self.handles.get(&handle).map(|e| e.rights)
    }

    /// 移除句柄，返回条目
    pub fn remove(&mut self, handle: Handle) -> Option<HandleEntry> {
        self.handles.remove(&handle)
    }

    /// 复制句柄（可削减权限）
    pub fn duplicate(&mut self, handle: Handle, new_rights: Rights) -> Option<Handle> {
        let entry = self.handles.get(&handle)?;
        if !entry.rights.contains(Rights::DUPLICATE) {
            return None;
        }
        let actual_rights = entry.rights & new_rights;
        let new_handle = Handle(self.next_id);
        self.next_id += 1;
        self.handles.insert(
            new_handle,
            HandleEntry {
                object: entry.object.clone(),
                rights: actual_rights,
            },
        );
        Some(new_handle)
    }

    /// 转移句柄（从当前表移除，返回对象和权限）
    pub fn transfer(&mut self, handle: Handle) -> Option<(Arc<dyn KernelObject>, Rights)> {
        let entry = self.handles.get(&handle)?;
        if !entry.rights.contains(Rights::TRANSFER) {
            return None;
        }
        let entry = self.handles.remove(&handle)?;
        Some((entry.object, entry.rights))
    }

    /// 批量转移句柄
    pub fn transfer_many(
        &mut self,
        handles: &[Handle],
    ) -> Option<Vec<(Arc<dyn KernelObject>, Rights)>> {
        // 先检查所有句柄是否可转移
        for &handle in handles {
            let entry = self.handles.get(&handle)?;
            if !entry.rights.contains(Rights::TRANSFER) {
                return None;
            }
        }

        // 全部可转移，执行转移
        let mut results = Vec::with_capacity(handles.len());
        for &handle in handles {
            if let Some(entry) = self.handles.remove(&handle) {
                results.push((entry.object, entry.rights));
            }
        }
        Some(results)
    }

    /// 接收转移过来的对象
    pub fn receive(&mut self, object: Arc<dyn KernelObject>, rights: Rights) -> Handle {
        self.insert(object, rights)
    }

    /// 批量接收
    pub fn receive_many(&mut self, objects: Vec<(Arc<dyn KernelObject>, Rights)>) -> Vec<Handle> {
        objects
            .into_iter()
            .map(|(obj, rights)| self.insert(obj, rights))
            .collect()
    }

    /// 句柄数量
    pub fn len(&self) -> usize {
        self.handles.len()
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.handles.is_empty()
    }

    /// 清空句柄表
    pub fn clear(&mut self) {
        self.handles.clear();
    }
}

impl Default for HandleTable {
    fn default() -> Self {
        Self::new()
    }
}
