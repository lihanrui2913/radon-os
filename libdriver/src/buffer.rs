//! 共享缓冲区管理

use alloc::vec::Vec;
use core::ops::{Deref, DerefMut};
use spin::Mutex;

use libradon::handle::Handle;
use libradon::memory::{map_vmo, unmap, MappingFlags, Vmo, VmoOptions};

use crate::{DriverError, Result};

/// 共享缓冲区
///
/// 可以在进程间共享的内存缓冲区。
pub struct SharedBuffer {
    vmo: Vmo,
    ptr: *mut u8,
    size: usize,
    /// 是否拥有映射
    owned: bool,
}

impl SharedBuffer {
    /// 创建新的共享缓冲区
    pub fn new(size: usize) -> Result<Self> {
        if size == 0 {
            return Err(DriverError::InvalidArgument);
        }

        let aligned_size = (size + 4095) & !4095;

        let vmo = Vmo::create(aligned_size, VmoOptions::COMMIT)?;

        let ptr = map_vmo(
            &vmo,
            0,
            aligned_size,
            MappingFlags::READ | MappingFlags::WRITE,
        )?;

        Ok(Self {
            vmo,
            ptr,
            size: aligned_size,
            owned: true,
        })
    }

    /// 从现有 VMO 创建（接收端使用）
    pub fn from_vmo(vmo: Vmo, size: usize) -> Result<Self> {
        let aligned_size = (size + 4095) & !4095;

        let ptr = map_vmo(
            &vmo,
            0,
            aligned_size,
            MappingFlags::READ | MappingFlags::WRITE,
        )?;

        Ok(Self {
            vmo,
            ptr,
            size: aligned_size,
            owned: true,
        })
    }

    /// 获取句柄（用于发送给其他进程）
    #[inline]
    pub fn handle(&self) -> Handle {
        self.vmo.handle()
    }

    /// 获取 VMO 引用
    #[inline]
    pub fn vmo(&self) -> &Vmo {
        &self.vmo
    }

    /// 获取指针
    #[inline]
    pub fn as_ptr(&self) -> *const u8 {
        self.ptr
    }

    /// 获取可变指针
    #[inline]
    pub fn as_mut_ptr(&mut self) -> *mut u8 {
        self.ptr
    }

    /// 获取大小
    #[inline]
    pub fn size(&self) -> usize {
        self.size
    }

    /// 作为切片
    #[inline]
    pub fn as_slice(&self) -> &[u8] {
        unsafe { core::slice::from_raw_parts(self.ptr, self.size) }
    }

    /// 作为可变切片
    #[inline]
    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        unsafe { core::slice::from_raw_parts_mut(self.ptr, self.size) }
    }

    /// 清零
    pub fn zero(&mut self) {
        unsafe {
            core::ptr::write_bytes(self.ptr, 0, self.size);
        }
    }
}

impl Drop for SharedBuffer {
    fn drop(&mut self) {
        if self.owned {
            let _ = unmap(self.ptr, self.size);
        }
    }
}

impl Deref for SharedBuffer {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        self.as_slice()
    }
}

impl DerefMut for SharedBuffer {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.as_mut_slice()
    }
}

unsafe impl Send for SharedBuffer {}
unsafe impl Sync for SharedBuffer {}

/// 缓冲区池
pub struct BufferPool {
    buffer_size: usize,
    max_buffers: usize,
    free_buffers: Mutex<Vec<SharedBuffer>>,
    total_allocated: Mutex<usize>,
}

impl BufferPool {
    /// 创建缓冲区池
    pub fn new(buffer_size: usize, max_buffers: usize) -> Self {
        Self {
            buffer_size,
            max_buffers,
            free_buffers: Mutex::new(Vec::with_capacity(max_buffers)),
            total_allocated: Mutex::new(0),
        }
    }

    /// 预分配缓冲区
    pub fn preallocate(&self, count: usize) -> Result<usize> {
        let mut allocated = 0;
        let mut free = self.free_buffers.lock();
        let mut total = self.total_allocated.lock();

        for _ in 0..count {
            if *total >= self.max_buffers {
                break;
            }

            match SharedBuffer::new(self.buffer_size) {
                Ok(buf) => {
                    free.push(buf);
                    *total += 1;
                    allocated += 1;
                }
                Err(e) => {
                    if allocated == 0 {
                        return Err(e);
                    }
                    break;
                }
            }
        }

        Ok(allocated)
    }

    /// 获取缓冲区
    pub fn acquire(&self) -> Result<PooledBuffer<'_>> {
        // 尝试从空闲列表获取
        if let Some(buf) = self.free_buffers.lock().pop() {
            return Ok(PooledBuffer {
                buffer: Some(buf),
                pool: self,
            });
        }

        // 尝试分配新的
        {
            let mut total = self.total_allocated.lock();
            if *total >= self.max_buffers {
                return Err(DriverError::OutOfMemory);
            }
            *total += 1;
        }

        let buf = SharedBuffer::new(self.buffer_size)?;
        Ok(PooledBuffer {
            buffer: Some(buf),
            pool: self,
        })
    }

    /// 释放缓冲区回池
    fn release(&self, mut buffer: SharedBuffer) {
        buffer.zero(); // 安全清零
        self.free_buffers.lock().push(buffer);
    }
}

/// 池化缓冲区
pub struct PooledBuffer<'a> {
    buffer: Option<SharedBuffer>,
    pool: &'a BufferPool,
}

impl<'a> PooledBuffer<'a> {
    /// 获取内部缓冲区引用
    pub fn buffer(&self) -> &SharedBuffer {
        self.buffer.as_ref().unwrap()
    }

    /// 获取内部缓冲区可变引用
    pub fn buffer_mut(&mut self) -> &mut SharedBuffer {
        self.buffer.as_mut().unwrap()
    }

    /// 获取句柄
    pub fn handle(&self) -> Handle {
        self.buffer().handle()
    }

    /// 释放并获取底层缓冲区（脱离池管理）
    pub fn detach(mut self) -> SharedBuffer {
        self.buffer.take().unwrap()
    }
}

impl<'a> Deref for PooledBuffer<'a> {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        self.buffer().as_slice()
    }
}

impl<'a> DerefMut for PooledBuffer<'a> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.buffer_mut().as_mut_slice()
    }
}

impl<'a> Drop for PooledBuffer<'a> {
    fn drop(&mut self) {
        if let Some(buf) = self.buffer.take() {
            self.pool.release(buf);
        }
    }
}
