use alloc::sync::Arc;
use alloc::vec::Vec;
use bitflags::bitflags;
use core::any::Any;
use rmm::{Arch, FrameAllocator, FrameCount, PhysicalAddress};
use spin::Mutex;

use crate::{
    EINVAL, Error, Result,
    arch::CurrentRmmArch,
    init::memory::{FRAME_ALLOCATOR, PAGE_SIZE},
};

use super::{KernelObject, ObjectType, SignalObserver, SignalState, Signals};

bitflags! {
    /// VMO 创建选项
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct VmoOptions: u32 {
        /// 立即分配物理内存（默认是按需分配）
        const COMMIT = 1 << 0;
        /// 物理连续（用于 DMA）
        const CONTIGUOUS = 1 << 1;
        /// 可调整大小
        const RESIZABLE = 1 << 2;
        /// 可丢弃（内存压力时可被回收）
        const DISCARDABLE = 1 << 3;
    }
}

bitflags! {
    /// VMO 操作权限
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct VmoRights: u32 {
        const READ = 1 << 0;
        const WRITE = 1 << 1;
        const EXECUTE = 1 << 2;
        const MAP = 1 << 3;
        const DUPLICATE = 1 << 4;
        const TRANSFER = 1 << 5;
        /// 可以获取物理地址（用于 DMA）
        const GET_PHYS = 1 << 6;
        /// 可以调整大小
        const RESIZE = 1 << 7;

        const DEFAULT = Self::READ.bits() | Self::WRITE.bits()
                      | Self::MAP.bits() | Self::DUPLICATE.bits()
                      | Self::TRANSFER.bits();
    }
}

/// 页面状态
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PageState {
    /// 未分配
    Uncommitted,
    /// 已分配
    Committed(PhysicalAddress, bool),
    /// 写时复制（指向父 VMO 的页面）
    CopyOnWrite { parent_offset: usize },
}

/// VMO 内部状态
struct VmoInner {
    /// 大小（字节，页对齐）
    size: usize,
    /// 页面状态数组
    pages: Vec<PageState>,
    /// 选项
    options: VmoOptions,
    /// 父 VMO（用于 COW）
    parent: Option<Arc<Vmo>>,
    /// 引用计数（用于共享统计）
    share_count: usize,
    /// 信号状态
    signal_state: SignalState,
}

/// Virtual Memory Object
pub struct Vmo {
    inner: Mutex<VmoInner>,
}

impl Vmo {
    /// 创建新的 VMO
    pub fn create(size: usize, options: VmoOptions) -> Result<Arc<Self>, VmoError> {
        if size == 0 {
            return Err(VmoError::InvalidSize);
        }

        // 页对齐
        let aligned_size = (size + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
        let page_count = aligned_size / PAGE_SIZE;

        let mut pages = Vec::with_capacity(page_count);

        if options.contains(VmoOptions::COMMIT) {
            // 立即分配所有页面
            if options.contains(VmoOptions::CONTIGUOUS) {
                // 分配连续物理内存
                let phys = unsafe {
                    FRAME_ALLOCATOR
                        .lock()
                        .allocate(FrameCount::new(page_count))
                        .ok_or(VmoError::NoMemory)?
                };

                let virt = unsafe { CurrentRmmArch::phys_to_virt(phys) };
                unsafe {
                    core::ptr::write_bytes(virt.data() as *mut u8, 0, page_count * PAGE_SIZE);
                }

                for i in 0..page_count {
                    pages.push(PageState::Committed(phys.add(i * PAGE_SIZE), true));
                }
            } else {
                // 分配非连续页面
                for _ in 0..page_count {
                    let phys = unsafe {
                        FRAME_ALLOCATOR
                            .lock()
                            .allocate_one()
                            .ok_or(VmoError::NoMemory)?
                    };

                    let virt = unsafe { CurrentRmmArch::phys_to_virt(phys) };
                    unsafe {
                        core::ptr::write_bytes(virt.data() as *mut u8, 0, PAGE_SIZE);
                    }

                    pages.push(PageState::Committed(phys, true));
                }
            }
        } else {
            // 按需分配
            pages.resize(page_count, PageState::Uncommitted);
        }

        Ok(Arc::new(Self {
            inner: Mutex::new(VmoInner {
                size: aligned_size,
                pages,
                options,
                parent: None,
                share_count: 1,
                signal_state: SignalState::new(),
            }),
        }))
    }

    /// 创建物理内存 VMO（用于 MMIO）
    pub fn create_physical(phys_addr: PhysicalAddress, size: usize) -> Result<Arc<Self>, VmoError> {
        if size == 0 {
            return Err(VmoError::InvalidSize);
        }

        let aligned_size = (size + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
        let page_count = aligned_size / PAGE_SIZE;

        let mut pages = Vec::with_capacity(page_count);
        for i in 0..page_count {
            pages.push(PageState::Committed(phys_addr.add(i * PAGE_SIZE), false));
        }

        Ok(Arc::new(Self {
            inner: Mutex::new(VmoInner {
                size: aligned_size,
                pages,
                options: VmoOptions::empty(),
                parent: None,
                share_count: 1,
                signal_state: SignalState::new(),
            }),
        }))
    }

    /// 创建 COW 克隆
    pub fn create_cow_clone(
        self: &Arc<Self>,
        offset: usize,
        size: usize,
    ) -> Result<Arc<Self>, VmoError> {
        let inner = self.inner.lock();

        if offset + size > inner.size {
            return Err(VmoError::OutOfRange);
        }

        let aligned_size = (size + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
        let page_count = aligned_size / PAGE_SIZE;
        let start_page = offset / PAGE_SIZE;

        let mut pages = Vec::with_capacity(page_count);
        for i in 0..page_count {
            pages.push(PageState::CopyOnWrite {
                parent_offset: (start_page + i) * PAGE_SIZE,
            });
        }

        drop(inner);

        Ok(Arc::new(Self {
            inner: Mutex::new(VmoInner {
                size: aligned_size,
                pages,
                options: VmoOptions::empty(),
                parent: Some(self.clone()),
                share_count: 1,
                signal_state: SignalState::new(),
            }),
        }))
    }

    /// 获取大小
    pub fn size(&self) -> usize {
        self.inner.lock().size
    }

    /// 调整大小
    pub fn resize(&self, new_size: usize) -> Result<(), VmoError> {
        let mut inner = self.inner.lock();

        if !inner.options.contains(VmoOptions::RESIZABLE) {
            return Err(VmoError::NotResizable);
        }

        let new_aligned = (new_size + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
        let new_page_count = new_aligned / PAGE_SIZE;
        let old_page_count = inner.pages.len();

        if new_page_count > old_page_count {
            // 扩展
            inner.pages.resize(new_page_count, PageState::Uncommitted);
        } else if new_page_count < old_page_count {
            // 收缩：释放多余页面
            for page in inner.pages.drain(new_page_count..) {
                if let PageState::Committed(phys, can_free) = page {
                    if can_free {
                        unsafe {
                            FRAME_ALLOCATOR.lock().free_one(phys);
                        }
                    }
                }
            }
        }

        inner.size = new_aligned;
        Ok(())
    }

    pub fn get_physical(&self) -> Result<usize> {
        let inner = self.inner.lock();
        if !inner.options.contains(VmoOptions::CONTIGUOUS) {
            return Err(Error::new(EINVAL));
        }

        if let PageState::Committed(phys, _) = inner.pages.get(0).ok_or(Error::new(EINVAL))? {
            Ok(phys.data())
        } else {
            Err(Error::new(EINVAL))
        }
    }

    /// 提交页面（分配物理内存）
    pub fn commit(&self, offset: usize, size: usize) -> Result<(), VmoError> {
        let mut inner = self.inner.lock();

        let start_page = offset / PAGE_SIZE;
        let end_page = (offset + size + PAGE_SIZE - 1) / PAGE_SIZE;

        if end_page > inner.pages.len() {
            return Err(VmoError::OutOfRange);
        }

        for i in start_page..end_page {
            if let PageState::Uncommitted = inner.pages[i] {
                let phys = unsafe {
                    FRAME_ALLOCATOR
                        .lock()
                        .allocate_one()
                        .ok_or(VmoError::NoMemory)?
                };

                // 清零
                unsafe {
                    let virt = CurrentRmmArch::phys_to_virt(phys);
                    core::ptr::write_bytes(virt.data() as *mut u8, 0, PAGE_SIZE);
                }

                inner.pages[i] = PageState::Committed(phys, true);
            }
        }

        Ok(())
    }

    /// 取消提交（释放物理内存）
    pub fn decommit(&self, offset: usize, size: usize) -> Result<(), VmoError> {
        let mut inner = self.inner.lock();

        let start_page = offset / PAGE_SIZE;
        let end_page = (offset + size + PAGE_SIZE - 1) / PAGE_SIZE;

        if end_page > inner.pages.len() {
            return Err(VmoError::OutOfRange);
        }

        for i in start_page..end_page {
            if let PageState::Committed(phys, can_free) = inner.pages[i] {
                if can_free {
                    unsafe {
                        FRAME_ALLOCATOR.lock().free_one(phys);
                    }
                    inner.pages[i] = PageState::Uncommitted;
                }
            }
        }

        Ok(())
    }

    /// 获取指定偏移的物理地址（可能触发分配或 COW）
    pub fn get_page(&self, offset: usize, write: bool) -> Result<PhysicalAddress, VmoError> {
        let mut inner = self.inner.lock();

        let page_index = offset / PAGE_SIZE;
        if page_index >= inner.pages.len() {
            return Err(VmoError::OutOfRange);
        }

        match inner.pages[page_index] {
            PageState::Committed(phys, _) => Ok(phys),

            PageState::Uncommitted => {
                // 按需分配
                let phys = unsafe {
                    FRAME_ALLOCATOR
                        .lock()
                        .allocate(FrameCount::new(1))
                        .ok_or(VmoError::NoMemory)?
                };

                // 清零
                unsafe {
                    let virt = CurrentRmmArch::phys_to_virt(phys);
                    core::ptr::write_bytes(virt.data() as *mut u8, 0, PAGE_SIZE);
                }

                inner.pages[page_index] = PageState::Committed(phys, true);
                Ok(phys)
            }

            PageState::CopyOnWrite { parent_offset } => {
                if write {
                    // 需要复制
                    let parent = inner.parent.as_ref().ok_or(VmoError::InvalidState)?;
                    let parent_phys = parent.get_page(parent_offset, false)?;

                    // 分配新页面
                    let new_phys = unsafe {
                        FRAME_ALLOCATOR
                            .lock()
                            .allocate(FrameCount::new(1))
                            .ok_or(VmoError::NoMemory)?
                    };

                    // 复制内容
                    unsafe {
                        let src = CurrentRmmArch::phys_to_virt(parent_phys);
                        let dst = CurrentRmmArch::phys_to_virt(new_phys);
                        core::ptr::copy_nonoverlapping(
                            src.data() as *const u8,
                            dst.data() as *mut u8,
                            PAGE_SIZE,
                        );
                    }

                    inner.pages[page_index] = PageState::Committed(new_phys, true);
                    Ok(new_phys)
                } else {
                    // 只读访问，返回父页面
                    let parent = inner.parent.as_ref().ok_or(VmoError::InvalidState)?;
                    parent.get_page(parent_offset, false)
                }
            }
        }
    }

    /// 读取数据
    pub fn read(&self, offset: usize, buf: &mut [u8]) -> Result<usize, VmoError> {
        let inner = self.inner.lock();

        if offset >= inner.size {
            return Ok(0);
        }

        let read_len = core::cmp::min(buf.len(), inner.size - offset);
        drop(inner);

        let mut bytes_read = 0;
        while bytes_read < read_len {
            let page_offset = (offset + bytes_read) % PAGE_SIZE;
            let chunk_len = core::cmp::min(PAGE_SIZE - page_offset, read_len - bytes_read);

            let phys = self.get_page(offset + bytes_read, false)?;

            unsafe {
                let src = CurrentRmmArch::phys_to_virt(phys).add(page_offset);
                core::ptr::copy_nonoverlapping(
                    src.data() as *const u8,
                    buf[bytes_read..].as_mut_ptr(),
                    chunk_len,
                );
            }

            bytes_read += chunk_len;
        }

        Ok(bytes_read)
    }

    /// 写入数据
    pub fn write(&self, offset: usize, buf: &[u8]) -> Result<usize, VmoError> {
        let inner = self.inner.lock();

        if offset >= inner.size {
            return Ok(0);
        }

        let write_len = core::cmp::min(buf.len(), inner.size - offset);
        drop(inner);

        let mut bytes_written = 0;
        while bytes_written < write_len {
            let page_offset = (offset + bytes_written) % PAGE_SIZE;
            let chunk_len = core::cmp::min(PAGE_SIZE - page_offset, write_len - bytes_written);

            let phys = self.get_page(offset + bytes_written, true)?;

            unsafe {
                let dst = CurrentRmmArch::phys_to_virt(phys).add(page_offset);
                core::ptr::copy_nonoverlapping(
                    buf[bytes_written..].as_ptr(),
                    dst.data() as *mut u8,
                    chunk_len,
                );
            }

            bytes_written += chunk_len;
        }

        Ok(bytes_written)
    }
}

impl Drop for Vmo {
    fn drop(&mut self) {
        let inner = self.inner.lock();

        // 只释放自己分配的页面（不释放 COW 指向的父页面）
        for page in &inner.pages {
            if let PageState::Committed(phys, can_free) = page {
                if *can_free {
                    unsafe {
                        FRAME_ALLOCATOR.lock().free_one(*phys);
                    }
                }
            }
        }
    }
}

impl KernelObject for Vmo {
    fn object_type(&self) -> ObjectType {
        ObjectType::Vmo
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

/// VMO 错误
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VmoError {
    InvalidSize,
    NoMemory,
    OutOfRange,
    NotResizable,
    InvalidState,
    AccessDenied,
}
