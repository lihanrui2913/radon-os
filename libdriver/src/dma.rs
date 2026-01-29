//! DMA 内存管理
//!
//! 提供物理连续内存的分配和管理，用于设备 DMA 操作。

use alloc::vec::Vec;
use core::ops::{Deref, DerefMut};
use core::ptr::NonNull;
use libradon::syscall::{self, nr};
use spin::Mutex;

use libradon::handle::Handle;
use libradon::memory::{map_vmo, unmap, MappingFlags, Vmo, VmoOptions};

use crate::{DriverError, Result};

/// 物理地址类型
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct PhysAddr(pub u64);

impl PhysAddr {
    pub const NULL: PhysAddr = PhysAddr(0);

    #[inline]
    pub fn new(addr: u64) -> Self {
        PhysAddr(addr)
    }

    #[inline]
    pub fn as_u64(&self) -> u64 {
        self.0
    }

    #[inline]
    pub fn add(&self, offset: usize) -> Self {
        PhysAddr(self.0 + offset as u64)
    }
}

/// DMA 内存区域
///
/// 提供物理连续的内存区域，可用于设备 DMA 操作。
pub struct DmaRegion {
    /// VMO 句柄
    vmo: Vmo,
    /// 虚拟地址
    virt_addr: NonNull<u8>,
    /// 物理地址
    phys_addr: PhysAddr,
    /// 大小
    size: usize,
}

impl DmaRegion {
    /// 分配 DMA 区域
    pub fn allocate(size: usize) -> Result<Self> {
        Self::allocate_aligned(size, 4096) // 默认页对齐
    }

    /// 分配指定对齐的 DMA 区域
    pub fn allocate_aligned(size: usize, alignment: usize) -> Result<Self> {
        if size == 0 {
            return Err(DriverError::InvalidArgument);
        }

        // 确保对齐至少是页大小
        let alignment = core::cmp::max(alignment, 4096);

        // 对齐大小
        let aligned_size = (size + alignment - 1) & !(alignment - 1);

        // 创建物理连续的 VMO
        let vmo = Vmo::create(aligned_size, VmoOptions::COMMIT | VmoOptions::CONTIGUOUS)?;

        // 映射到虚拟地址空间
        let ptr = map_vmo(
            &vmo,
            0,
            aligned_size,
            MappingFlags::READ | MappingFlags::WRITE,
        )?;

        // 获取物理地址
        let phys_addr = Self::get_phys_addr(&vmo)?;

        Ok(Self {
            vmo,
            virt_addr: NonNull::new(ptr).ok_or(DriverError::OutOfMemory)?,
            phys_addr,
            size: aligned_size,
        })
    }

    /// 获取 VMO 的物理地址（需要内核支持）
    fn get_phys_addr(vmo: &Vmo) -> Result<PhysAddr> {
        let ret = unsafe { syscall::syscall1(nr::SYS_VMO_GET_PHYS, vmo.handle().raw() as usize) };
        Ok(PhysAddr::new(ret as u64))
    }

    /// 获取虚拟地址
    #[inline]
    pub fn virt_addr(&self) -> *mut u8 {
        self.virt_addr.as_ptr()
    }

    /// 获取物理地址
    #[inline]
    pub fn phys_addr(&self) -> PhysAddr {
        self.phys_addr
    }

    /// 获取大小
    #[inline]
    pub fn size(&self) -> usize {
        self.size
    }

    /// 获取 VMO 句柄（用于共享）
    #[inline]
    pub fn vmo(&self) -> &Vmo {
        &self.vmo
    }

    /// 获取 VMO 句柄值
    #[inline]
    pub fn handle(&self) -> Handle {
        self.vmo.handle()
    }

    /// 作为字节切片
    #[inline]
    pub fn as_slice(&self) -> &[u8] {
        unsafe { core::slice::from_raw_parts(self.virt_addr.as_ptr(), self.size) }
    }

    /// 作为可变字节切片
    #[inline]
    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        unsafe { core::slice::from_raw_parts_mut(self.virt_addr.as_ptr(), self.size) }
    }

    /// 作为类型化引用
    #[inline]
    pub fn as_ref<T>(&self) -> Option<&T> {
        if self.size >= core::mem::size_of::<T>() {
            Some(unsafe { &*(self.virt_addr.as_ptr() as *const T) })
        } else {
            None
        }
    }

    /// 作为类型化可变引用
    #[inline]
    pub fn as_mut<T>(&mut self) -> Option<&mut T> {
        if self.size >= core::mem::size_of::<T>() {
            Some(unsafe { &mut *(self.virt_addr.as_ptr() as *mut T) })
        } else {
            None
        }
    }

    /// 清零
    pub fn zero(&mut self) {
        unsafe {
            core::ptr::write_bytes(self.virt_addr.as_ptr(), 0, self.size);
        }
    }

    /// 从偏移处获取子区域的物理地址
    #[inline]
    pub fn phys_addr_at(&self, offset: usize) -> Option<PhysAddr> {
        if offset < self.size {
            Some(self.phys_addr.add(offset))
        } else {
            None
        }
    }
}

impl Drop for DmaRegion {
    fn drop(&mut self) {
        let _ = unmap(self.virt_addr.as_ptr(), self.size);
    }
}

// 实现 Send 和 Sync
unsafe impl Send for DmaRegion {}
unsafe impl Sync for DmaRegion {}

/// DMA 缓冲区
///
/// 简单的 DMA 缓冲区封装，带读写位置跟踪。
pub struct DmaBuffer {
    region: DmaRegion,
    /// 读位置
    read_pos: usize,
    /// 写位置
    write_pos: usize,
}

impl DmaBuffer {
    /// 创建新的 DMA 缓冲区
    pub fn new(size: usize) -> Result<Self> {
        let mut region = DmaRegion::allocate(size)?;
        region.zero();

        Ok(Self {
            region,
            read_pos: 0,
            write_pos: 0,
        })
    }

    /// 获取底层 DMA 区域
    #[inline]
    pub fn region(&self) -> &DmaRegion {
        &self.region
    }

    /// 获取物理地址
    #[inline]
    pub fn phys_addr(&self) -> PhysAddr {
        self.region.phys_addr()
    }

    /// 获取大小
    #[inline]
    pub fn size(&self) -> usize {
        self.region.size()
    }

    /// 可读字节数
    #[inline]
    pub fn readable(&self) -> usize {
        self.write_pos - self.read_pos
    }

    /// 可写字节数
    #[inline]
    pub fn writable(&self) -> usize {
        self.region.size() - self.write_pos
    }

    /// 写入数据
    pub fn write(&mut self, data: &[u8]) -> usize {
        let to_write = core::cmp::min(data.len(), self.writable());
        if to_write > 0 {
            unsafe {
                core::ptr::copy_nonoverlapping(
                    data.as_ptr(),
                    self.region.virt_addr().add(self.write_pos),
                    to_write,
                );
            }
            self.write_pos += to_write;
        }
        to_write
    }

    /// 读取数据
    pub fn read(&mut self, buf: &mut [u8]) -> usize {
        let to_read = core::cmp::min(buf.len(), self.readable());
        if to_read > 0 {
            unsafe {
                core::ptr::copy_nonoverlapping(
                    self.region.virt_addr().add(self.read_pos),
                    buf.as_mut_ptr(),
                    to_read,
                );
            }
            self.read_pos += to_read;
        }
        to_read
    }

    /// 重置位置
    pub fn reset(&mut self) {
        self.read_pos = 0;
        self.write_pos = 0;
    }

    /// 获取 VMO 句柄用于共享
    #[inline]
    pub fn handle(&self) -> Handle {
        self.region.handle()
    }
}

/// DMA 内存池
///
/// 管理多个 DMA 缓冲区，支持复用。
pub struct DmaPool {
    /// 缓冲区大小
    buffer_size: usize,
    /// 空闲缓冲区列表
    free_buffers: Mutex<Vec<DmaRegion>>,
    /// 已分配数量
    allocated: Mutex<usize>,
    /// 最大缓冲区数量
    max_buffers: usize,
}

impl DmaPool {
    /// 创建 DMA 池
    pub fn new(buffer_size: usize, max_buffers: usize) -> Self {
        Self {
            buffer_size,
            free_buffers: Mutex::new(Vec::new()),
            allocated: Mutex::new(0),
            max_buffers,
        }
    }

    /// 预分配缓冲区
    pub fn preallocate(&self, count: usize) -> Result<()> {
        let mut free = self.free_buffers.lock();
        let mut allocated = self.allocated.lock();

        for _ in 0..count {
            if *allocated >= self.max_buffers {
                break;
            }

            let region = DmaRegion::allocate(self.buffer_size)?;
            free.push(region);
            *allocated += 1;
        }

        Ok(())
    }

    /// 获取缓冲区
    pub fn acquire(&self) -> Result<PooledDmaBuffer<'_>> {
        // 尝试从空闲列表获取
        if let Some(region) = self.free_buffers.lock().pop() {
            return Ok(PooledDmaBuffer {
                region: Some(region),
                pool: self,
            });
        }

        // 检查是否可以分配新的
        {
            let mut allocated = self.allocated.lock();
            if *allocated >= self.max_buffers {
                return Err(DriverError::OutOfMemory);
            }
            *allocated += 1;
        }

        // 分配新的
        let region = DmaRegion::allocate(self.buffer_size)?;
        Ok(PooledDmaBuffer {
            region: Some(region),
            pool: self,
        })
    }

    /// 释放缓冲区回池
    fn release(&self, region: DmaRegion) {
        self.free_buffers.lock().push(region);
    }

    /// 获取统计信息
    pub fn stats(&self) -> PoolStats {
        PoolStats {
            buffer_size: self.buffer_size,
            free_count: self.free_buffers.lock().len(),
            allocated: *self.allocated.lock(),
            max_buffers: self.max_buffers,
        }
    }
}

/// 池化的 DMA 缓冲区
///
/// 释放时自动归还到池中。
pub struct PooledDmaBuffer<'a> {
    region: Option<DmaRegion>,
    pool: &'a DmaPool,
}

impl<'a> PooledDmaBuffer<'a> {
    #[inline]
    pub fn region(&self) -> &DmaRegion {
        self.region.as_ref().unwrap()
    }

    #[inline]
    pub fn region_mut(&mut self) -> &mut DmaRegion {
        self.region.as_mut().unwrap()
    }

    #[inline]
    pub fn phys_addr(&self) -> PhysAddr {
        self.region().phys_addr()
    }
}

impl<'a> Deref for PooledDmaBuffer<'a> {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        self.region().as_slice()
    }
}

impl<'a> DerefMut for PooledDmaBuffer<'a> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.region_mut().as_mut_slice()
    }
}

impl<'a> Drop for PooledDmaBuffer<'a> {
    fn drop(&mut self) {
        if let Some(region) = self.region.take() {
            self.pool.release(region);
        }
    }
}

/// 池统计信息
#[derive(Debug, Clone, Copy)]
pub struct PoolStats {
    pub buffer_size: usize,
    pub free_count: usize,
    pub allocated: usize,
    pub max_buffers: usize,
}

/// Scatter-Gather 描述符
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SgDescriptor {
    /// 物理地址
    pub phys_addr: u64,
    /// 长度
    pub length: u32,
    /// 标志
    pub flags: u32,
}

impl SgDescriptor {
    pub const FLAG_LAST: u32 = 1 << 0;
    pub const FLAG_INTERRUPT: u32 = 1 << 1;

    pub fn new(phys_addr: PhysAddr, length: u32) -> Self {
        Self {
            phys_addr: phys_addr.as_u64(),
            length,
            flags: 0,
        }
    }

    pub fn with_flags(mut self, flags: u32) -> Self {
        self.flags = flags;
        self
    }
}

/// Scatter-Gather 列表
pub struct SgList {
    descriptors: Vec<SgDescriptor>,
    /// 描述符表的 DMA 区域（用于硬件访问）
    desc_table: Option<DmaRegion>,
}

impl SgList {
    pub fn new() -> Self {
        Self {
            descriptors: Vec::new(),
            desc_table: None,
        }
    }

    /// 添加描述符
    pub fn push(&mut self, phys_addr: PhysAddr, length: u32) {
        self.descriptors.push(SgDescriptor::new(phys_addr, length));
        self.desc_table = None; // 使缓存失效
    }

    /// 添加 DMA 区域
    pub fn push_region(&mut self, region: &DmaRegion, offset: usize, length: usize) {
        if let Some(phys) = region.phys_addr_at(offset) {
            self.push(phys, length as u32);
        }
    }

    /// 描述符数量
    pub fn len(&self) -> usize {
        self.descriptors.len()
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.descriptors.is_empty()
    }

    /// 获取描述符
    pub fn get(&self, index: usize) -> Option<&SgDescriptor> {
        self.descriptors.get(index)
    }

    /// 准备硬件可访问的描述符表
    pub fn prepare_hw_table(&mut self) -> Result<PhysAddr> {
        if self.desc_table.is_some() {
            return Ok(self.desc_table.as_ref().unwrap().phys_addr());
        }

        let table_size = self.descriptors.len() * core::mem::size_of::<SgDescriptor>();
        let region = DmaRegion::allocate(table_size)?;

        // 复制描述符到 DMA 区域
        unsafe {
            core::ptr::copy_nonoverlapping(
                self.descriptors.as_ptr(),
                region.virt_addr() as *mut SgDescriptor,
                self.descriptors.len(),
            );
        }

        // 标记最后一个
        if !self.descriptors.is_empty() {
            let last_idx = self.descriptors.len() - 1;
            let descs = unsafe {
                core::slice::from_raw_parts_mut(
                    region.virt_addr() as *mut SgDescriptor,
                    self.descriptors.len(),
                )
            };
            descs[last_idx].flags |= SgDescriptor::FLAG_LAST;
        }

        let phys = region.phys_addr();
        self.desc_table = Some(region);

        Ok(phys)
    }

    /// 清空
    pub fn clear(&mut self) {
        self.descriptors.clear();
        self.desc_table = None;
    }

    /// 总长度
    pub fn total_length(&self) -> usize {
        self.descriptors.iter().map(|d| d.length as usize).sum()
    }
}
