use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;
use bitflags::bitflags;
use core::any::Any;
use rmm::{PageFlags, PageMapper, PhysicalAddress, VirtualAddress};
use spin::Mutex;

use crate::{
    arch::CurrentRmmArch,
    init::memory::{FRAME_ALLOCATOR, PAGE_SIZE, align_down, align_up},
};

use super::{KernelObject, ObjectType, SignalObserver, SignalState, Signals, vmo::Vmo};

bitflags! {
    /// 映射权限
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct MappingFlags: u32 {
        const READ = 1 << 0;
        const WRITE = 1 << 1;
        const EXECUTE = 1 << 2;
        /// 映射到特定地址
        const SPECIFIC = 1 << 3;
        /// 允许地址偏移（用于 ASLR）
        const OFFSET_IS_UPPER_LIMIT = 1 << 4;
    }
}

/// 映射信息
#[derive(Clone)]
pub struct Mapping {
    /// VMO
    pub vmo: Arc<Vmo>,
    /// VMO 内偏移
    pub vmo_offset: usize,
    /// 映射大小
    pub size: usize,
    /// 权限
    pub flags: MappingFlags,
}

/// VMAR 内部状态
struct VmarInner {
    /// 基地址
    base: VirtualAddress,
    /// 大小
    size: usize,
    /// 是否为根 VMAR
    is_root: bool,
    /// 映射表 (虚拟地址 -> 映射)
    mappings: BTreeMap<usize, Mapping>,
    /// 子 VMAR
    children: Vec<Arc<Vmar>>,
    /// 下一个可用地址（简化分配）
    next_alloc: usize,
    /// 信号状态
    signal_state: SignalState,
    /// 页表（对于根 VMAR）
    page_table: Option<PhysicalAddress>,
}

/// Virtual Memory Address Region
pub struct Vmar {
    inner: Mutex<VmarInner>,
}

impl Vmar {
    /// 创建根 VMAR（进程的整个用户地址空间）
    pub fn create_root(
        base: VirtualAddress,
        size: usize,
        next_alloc: usize,
        page_table: PhysicalAddress,
    ) -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(VmarInner {
                base,
                size,
                is_root: true,
                mappings: BTreeMap::new(),
                children: Vec::new(),
                next_alloc,
                signal_state: SignalState::new(),
                page_table: Some(page_table),
            }),
        })
    }

    /// 创建子 VMAR
    pub fn create_child(&self, offset: usize, size: usize) -> Result<Arc<Vmar>, VmarError> {
        let mut inner = self.inner.lock();

        let child_base = inner.base.add(offset);

        // 检查范围
        if offset + size > inner.size {
            return Err(VmarError::OutOfRange);
        }

        // TODO: 检查是否与现有映射或子 VMAR 重叠

        let child = Arc::new(Vmar {
            inner: Mutex::new(VmarInner {
                base: child_base,
                size,
                is_root: false,
                mappings: BTreeMap::new(),
                children: Vec::new(),
                next_alloc: child_base.data(),
                signal_state: SignalState::new(),
                page_table: inner.page_table,
            }),
        });

        inner.children.push(child.clone());

        Ok(child)
    }

    pub fn page_table_addr(&self) -> Option<PhysicalAddress> {
        self.inner.lock().page_table
    }

    /// 映射 VMO
    pub fn map(
        &self,
        vmo: Arc<Vmo>,
        vmo_offset: usize,
        size: usize,
        flags: MappingFlags,
        vaddr: Option<VirtualAddress>,
    ) -> Result<VirtualAddress, VmarError> {
        let mut inner = self.inner.lock();

        // 对齐检查
        let aligned_size = align_up(size);

        // 确定虚拟地址
        let map_addr = if let Some(addr) = vaddr {
            if !flags.contains(MappingFlags::SPECIFIC) {
                return Err(VmarError::InvalidArgs);
            }

            // 检查地址是否在范围内
            if addr.data() < inner.base.data()
                || addr.data() + aligned_size > inner.base.data() + inner.size
            {
                return Err(VmarError::OutOfRange);
            }

            addr
        } else {
            // 自动分配地址
            let addr = VirtualAddress::new(inner.next_alloc);

            // 检查是否有足够空间
            if inner.next_alloc + aligned_size > inner.base.data() + inner.size {
                return Err(VmarError::NoSpace);
            }

            inner.next_alloc += aligned_size;
            addr
        };

        // 检查是否与现有映射重叠
        for (&existing_addr, mapping) in &inner.mappings {
            let existing_end = existing_addr + mapping.size;
            let new_end = map_addr.data() + aligned_size;

            if !(new_end <= existing_addr || map_addr.data() >= existing_end) {
                return Err(VmarError::Overlap);
            }
        }

        // 创建页表映射
        if let Some(page_table) = inner.page_table {
            let page_count = aligned_size / PAGE_SIZE;

            for i in 0..page_count {
                let virt = map_addr.add(i * PAGE_SIZE);

                // 获取物理页面
                let phys = vmo
                    .get_page(vmo_offset + i * PAGE_SIZE, false)
                    .map_err(|_| VmarError::VmoError)?;

                // 设置页表项
                unsafe {
                    map_page(page_table, virt, phys, flags);
                }
            }
        }

        // 保存映射信息
        inner.mappings.insert(
            map_addr.data(),
            Mapping {
                vmo,
                vmo_offset,
                size: aligned_size,
                flags,
            },
        );

        Ok(map_addr)
    }

    /// 解除映射
    pub fn unmap(&self, addr: VirtualAddress, size: usize) -> Result<(), VmarError> {
        let mut inner = self.inner.lock();

        let aligned_size = align_up(size);

        // 查找映射
        let mapping = inner
            .mappings
            .remove(&addr.data())
            .ok_or(VmarError::NotMapped)?;

        if mapping.size != aligned_size {
            // 部分解除映射（复杂，暂不支持）
            inner.mappings.insert(addr.data(), mapping);
            return Err(VmarError::InvalidArgs);
        }

        // 清除页表项
        if let Some(page_table) = inner.page_table {
            let page_count = aligned_size / PAGE_SIZE;

            for i in 0..page_count {
                let virt = addr.add(i * PAGE_SIZE);
                unsafe {
                    unmap_page(page_table, virt);
                }
            }
        }

        Ok(())
    }

    /// 修改映射权限
    pub fn protect(
        &self,
        addr: VirtualAddress,
        _size: usize,
        flags: MappingFlags,
    ) -> Result<(), VmarError> {
        let mut inner = self.inner.lock();

        let page_table = inner.page_table;

        let mapping = inner
            .mappings
            .get_mut(&addr.data())
            .ok_or(VmarError::NotMapped)?;

        // 更新权限
        mapping.flags = flags;

        // 更新页表
        if let Some(page_table) = page_table {
            let page_count = mapping.size / PAGE_SIZE;

            for i in 0..page_count {
                let virt = addr.add(i * PAGE_SIZE);
                unsafe {
                    update_page_flags(page_table, virt, flags);
                }
            }
        }

        Ok(())
    }

    /// 获取基地址
    pub fn base(&self) -> VirtualAddress {
        self.inner.lock().base
    }

    /// 获取大小
    pub fn size(&self) -> usize {
        self.inner.lock().size
    }

    /// 处理缺页异常
    pub fn handle_page_fault(&self, addr: VirtualAddress, write: bool) -> Result<(), VmarError> {
        let inner = self.inner.lock();

        // 查找包含该地址的映射
        for (&base, mapping) in &inner.mappings {
            if addr.data() >= base && addr.data() < base + mapping.size {
                // 检查权限
                if write && !mapping.flags.contains(MappingFlags::WRITE) {
                    return Err(VmarError::AccessDenied);
                }

                // 计算偏移
                let offset_in_mapping = addr.data() - base;
                let page_offset = align_down(offset_in_mapping);

                // 获取物理页面（可能触发 COW）
                let phys = mapping
                    .vmo
                    .get_page(mapping.vmo_offset + page_offset, write)
                    .map_err(|_| VmarError::VmoError)?;

                // 更新页表
                if let Some(page_table) = inner.page_table {
                    let virt = VirtualAddress::new(base + page_offset);
                    unsafe {
                        map_page(page_table, virt, phys, mapping.flags);
                    }
                }

                return Ok(());
            }
        }

        Err(VmarError::NotMapped)
    }
}

impl KernelObject for Vmar {
    fn object_type(&self) -> ObjectType {
        ObjectType::Vmar
    }

    fn signals(&self) -> Signals {
        self.inner.lock().signal_state.get()
    }

    fn signal_set(&self, signals: Signals) {
        self.inner.lock().signal_state.set(signals);
    }

    fn signal_clear(&self, signals: Signals) {
        self.inner.lock().signal_state.clear(signals);
    }

    fn add_signal_observer(&self, observer: SignalObserver) {
        self.inner.lock().signal_state.add_observer(observer);
    }

    fn remove_signal_observer(&self, key: u64) {
        self.inner.lock().signal_state.remove_observer(key);
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// VMAR 错误
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VmarError {
    InvalidArgs,
    OutOfRange,
    NoSpace,
    Overlap,
    NotMapped,
    VmoError,
    AccessDenied,
}

// 页表操作（架构相关，需要根据实际实现）
unsafe fn map_page(
    page_table: PhysicalAddress,
    virt: VirtualAddress,
    phys: PhysicalAddress,
    flags: MappingFlags,
) {
    let mut frame_allocator = FRAME_ALLOCATOR.lock();
    let mut mapper =
        unsafe { PageMapper::new(rmm::TableKind::User, page_table, &mut *frame_allocator) };
    let page_flags = PageFlags::<CurrentRmmArch>::new()
        .execute(flags.contains(MappingFlags::EXECUTE))
        .write(flags.contains(MappingFlags::WRITE))
        .user(true);
    if let Some(flusher) = mapper.map_phys(virt, phys, page_flags) {
        flusher.flush();
    }
}

unsafe fn unmap_page(page_table: PhysicalAddress, virt: VirtualAddress) {
    let mut frame_allocator = FRAME_ALLOCATOR.lock();
    let mut mapper = unsafe {
        PageMapper::<CurrentRmmArch, _>::new(
            rmm::TableKind::User,
            page_table,
            &mut *frame_allocator,
        )
    };
    if let Some((_phys, _flags, flusher)) = mapper.unmap_phys(virt, true) {
        flusher.flush();
    }
}

unsafe fn update_page_flags(
    page_table: PhysicalAddress,
    virt: VirtualAddress,
    flags: MappingFlags,
) {
    let mut frame_allocator = FRAME_ALLOCATOR.lock();
    let mut mapper = unsafe {
        PageMapper::<CurrentRmmArch, _>::new(
            rmm::TableKind::User,
            page_table,
            &mut *frame_allocator,
        )
    };
    let page_flags = PageFlags::<CurrentRmmArch>::new()
        .execute(flags.contains(MappingFlags::EXECUTE))
        .write(flags.contains(MappingFlags::WRITE))
        .user(true);
    if let Some((_flags, _addr, flusher)) = mapper.remap_with(virt, |_| page_flags) {
        flusher.flush();
    }
}
