//! 环形缓冲区
//!
//! 用于驱动程序和设备之间的高效通信。

use core::sync::atomic::Ordering;

use crate::dma::DmaRegion;
use crate::{DriverError, PhysAddr, Result};

/// 描述符
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct Descriptor {
    /// 缓冲区物理地址
    pub addr: u64,
    /// 长度
    pub len: u32,
    /// 标志
    pub flags: u16,
    /// 下一个描述符索引
    pub next: u16,
}

impl Descriptor {
    pub const FLAG_NEXT: u16 = 1 << 0;
    pub const FLAG_WRITE: u16 = 1 << 1; // 设备写入
    pub const FLAG_INDIRECT: u16 = 1 << 2;

    pub fn new(addr: PhysAddr, len: u32) -> Self {
        Self {
            addr: addr.as_u64(),
            len,
            flags: 0,
            next: 0,
        }
    }

    pub fn with_flags(mut self, flags: u16) -> Self {
        self.flags = flags;
        self
    }

    pub fn with_next(mut self, next: u16) -> Self {
        self.flags |= Self::FLAG_NEXT;
        self.next = next;
        self
    }
}

/// 环形缓冲区布局
#[repr(C)]
struct RingLayout {
    /// 描述符表
    descs: *mut Descriptor,
    /// 可用环
    avail: *mut AvailRing,
    /// 已用环
    used: *mut UsedRing,
}

/// 可用环
#[repr(C)]
struct AvailRing {
    flags: u16,
    idx: u16,
    // ring: [u16; size] - 变长数组
}

/// 已用环
#[repr(C)]
struct UsedRing {
    flags: u16,
    idx: u16,
    // ring: [UsedElem; size] - 变长数组
}

/// 已用元素
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct UsedElem {
    pub id: u32,
    pub len: u32,
}

/// 环形缓冲区
///
/// 类似 Virtio 风格的环形缓冲区实现。
pub struct RingBuffer {
    /// DMA 区域
    region: DmaRegion,
    /// 描述符数量
    size: u16,
    /// 空闲描述符链表头
    free_head: u16,
    /// 空闲描述符数量
    free_count: u16,
    /// 上次看到的已用索引
    last_used_idx: u16,
    /// 布局偏移
    desc_offset: usize,
    avail_offset: usize,
    used_offset: usize,
}

impl RingBuffer {
    /// 创建环形缓冲区
    pub fn new(size: u16) -> Result<Self> {
        if size == 0 || !size.is_power_of_two() {
            return Err(DriverError::InvalidArgument);
        }

        // 计算各部分大小
        let desc_size = (size as usize) * core::mem::size_of::<Descriptor>();
        let avail_size = 4 + (size as usize) * 2 + 2; // flags + idx + ring + used_event
        let used_size = 4 + (size as usize) * core::mem::size_of::<UsedElem>() + 2;

        // 对齐
        let avail_offset = desc_size;
        let used_offset = (avail_offset + avail_size + 4095) & !4095; // 页对齐
        let total_size = used_offset + used_size;

        let mut region = DmaRegion::allocate(total_size)?;
        region.zero();

        let mut ring = Self {
            region,
            size,
            free_head: 0,
            free_count: size,
            last_used_idx: 0,
            desc_offset: 0,
            avail_offset,
            used_offset,
        };

        // 初始化空闲链表
        ring.init_free_list();

        Ok(ring)
    }

    /// 初始化空闲链表
    fn init_free_list(&mut self) {
        for i in 0..(self.size - 1) {
            self.desc_mut(i).next = i + 1;
        }
        self.desc_mut(self.size - 1).next = 0xFFFF; // 链表尾
    }

    /// 获取描述符引用
    fn desc(&self, idx: u16) -> &Descriptor {
        unsafe {
            &*((self.region.virt_addr() as usize + self.desc_offset) as *const Descriptor)
                .add(idx as usize)
        }
    }

    /// 获取描述符可变引用
    fn desc_mut(&mut self, idx: u16) -> &mut Descriptor {
        unsafe {
            &mut *((self.region.virt_addr() as usize + self.desc_offset) as *mut Descriptor)
                .add(idx as usize)
        }
    }

    /// 获取可用环
    fn avail(&self) -> &AvailRing {
        unsafe { &*((self.region.virt_addr() as usize + self.avail_offset) as *const AvailRing) }
    }

    /// 获取可用环可变引用
    fn avail_mut(&mut self) -> &mut AvailRing {
        unsafe { &mut *((self.region.virt_addr() as usize + self.avail_offset) as *mut AvailRing) }
    }

    /// 获取可用环 ring 数组
    fn avail_ring(&self) -> &[u16] {
        unsafe {
            let ptr = (self.region.virt_addr() as usize + self.avail_offset + 4) as *const u16;
            core::slice::from_raw_parts(ptr, self.size as usize)
        }
    }

    /// 获取可用环 ring 数组可变引用
    fn avail_ring_mut(&mut self) -> &mut [u16] {
        unsafe {
            let ptr = (self.region.virt_addr() as usize + self.avail_offset + 4) as *mut u16;
            core::slice::from_raw_parts_mut(ptr, self.size as usize)
        }
    }

    /// 获取已用环
    fn used(&self) -> &UsedRing {
        unsafe { &*((self.region.virt_addr() as usize + self.used_offset) as *const UsedRing) }
    }

    /// 获取已用环 ring 数组
    fn used_ring(&self) -> &[UsedElem] {
        unsafe {
            let ptr = (self.region.virt_addr() as usize + self.used_offset + 4) as *const UsedElem;
            core::slice::from_raw_parts(ptr, self.size as usize)
        }
    }

    /// 分配描述符
    pub fn alloc_desc(&mut self) -> Option<u16> {
        if self.free_count == 0 {
            return None;
        }

        let idx = self.free_head;
        self.free_head = self.desc(idx).next;
        self.free_count -= 1;

        Some(idx)
    }

    /// 释放描述符
    pub fn free_desc(&mut self, idx: u16) {
        self.desc_mut(idx).next = self.free_head;
        self.free_head = idx;
        self.free_count += 1;
    }

    /// 释放描述符链
    pub fn free_chain(&mut self, head: u16) {
        let mut idx = head;
        loop {
            let desc = *self.desc(idx);
            self.free_desc(idx);

            if desc.flags & Descriptor::FLAG_NEXT == 0 {
                break;
            }
            idx = desc.next;
        }
    }

    /// 添加缓冲区到可用环
    pub fn push_avail(&mut self, desc_head: u16) {
        let avail_idx = self.avail().idx;
        let ring_idx = (avail_idx % self.size) as usize;
        self.avail_ring_mut()[ring_idx] = desc_head;

        // 内存屏障
        core::sync::atomic::fence(Ordering::Release);

        // 更新索引
        self.avail_mut().idx = avail_idx.wrapping_add(1);
    }

    /// 从已用环弹出
    pub fn pop_used(&mut self) -> Option<UsedElem> {
        let used_idx = self.used().idx;

        if self.last_used_idx == used_idx {
            return None;
        }

        // 内存屏障
        core::sync::atomic::fence(Ordering::Acquire);

        let ring_idx = (self.last_used_idx % self.size) as usize;
        let elem = self.used_ring()[ring_idx];
        self.last_used_idx = self.last_used_idx.wrapping_add(1);

        Some(elem)
    }

    /// 检查是否有已完成的缓冲区
    pub fn has_used(&self) -> bool {
        self.last_used_idx != self.used().idx
    }

    /// 获取描述符表物理地址
    pub fn desc_phys(&self) -> PhysAddr {
        self.region.phys_addr()
    }

    /// 获取可用环物理地址
    pub fn avail_phys(&self) -> PhysAddr {
        self.region.phys_addr().add(self.avail_offset)
    }

    /// 获取已用环物理地址
    pub fn used_phys(&self) -> PhysAddr {
        self.region.phys_addr().add(self.used_offset)
    }

    /// 获取大小
    pub fn size(&self) -> u16 {
        self.size
    }

    /// 空闲描述符数量
    pub fn free_count(&self) -> u16 {
        self.free_count
    }

    /// 添加单个缓冲区
    pub fn add_buffer(&mut self, addr: PhysAddr, len: u32, write: bool) -> Option<u16> {
        let idx = self.alloc_desc()?;

        let mut flags = 0;
        if write {
            flags |= Descriptor::FLAG_WRITE;
        }

        *self.desc_mut(idx) = Descriptor {
            addr: addr.as_u64(),
            len,
            flags,
            next: 0,
        };

        self.push_avail(idx);

        Some(idx)
    }

    /// 添加描述符链
    pub fn add_buffer_chain(
        &mut self,
        buffers: &[(PhysAddr, u32, bool)], // (addr, len, write)
    ) -> Option<u16> {
        if buffers.is_empty() || buffers.len() > self.free_count as usize {
            return None;
        }

        let mut head = None;
        let mut prev_idx = None;

        for (i, &(addr, len, write)) in buffers.iter().enumerate() {
            let idx = self.alloc_desc()?;

            if head.is_none() {
                head = Some(idx);
            }

            let mut flags = 0;
            if write {
                flags |= Descriptor::FLAG_WRITE;
            }
            if i < buffers.len() - 1 {
                flags |= Descriptor::FLAG_NEXT;
            }

            *self.desc_mut(idx) = Descriptor {
                addr: addr.as_u64(),
                len,
                flags,
                next: 0,
            };

            if let Some(prev) = prev_idx {
                self.desc_mut(prev).next = idx;
            }
            prev_idx = Some(idx);
        }

        let head_idx = head.unwrap();
        self.push_avail(head_idx);

        Some(head_idx)
    }
}

unsafe impl Send for RingBuffer {}
unsafe impl Sync for RingBuffer {}
