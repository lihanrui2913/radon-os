//! MMIO 内存映射

use core::marker::PhantomData;
use core::ptr::{read_volatile, write_volatile};

use libradon::memory::{map_vmo, MappingFlags, Vmo};

use crate::{DriverError, PhysAddr, Result};

/// MMIO 区域
///
/// 映射设备寄存器到用户空间。
pub struct MmioRegion {
    /// VMO
    vmo: Vmo,
    /// 虚拟地址
    base: *mut u8,
    /// 大小
    size: usize,
    /// 物理地址
    phys_addr: PhysAddr,
}

impl MmioRegion {
    /// 映射 MMIO 区域
    ///
    /// # 安全性
    /// 调用者必须确保物理地址范围对应有效的设备内存。
    pub unsafe fn map(phys_addr: PhysAddr, size: usize) -> Result<Self> {
        if size == 0 {
            return Err(DriverError::InvalidArgument);
        }

        // 对齐到页边界
        let page_offset = (phys_addr.as_u64() as usize) & 0xFFF;
        let aligned_phys = PhysAddr::new(phys_addr.as_u64() & !0xFFF);
        let aligned_size = (size + page_offset + 0xFFF) & !0xFFF;

        // 创建物理内存 VMO
        // 注意：需要特殊权限
        let vmo = Vmo::create_physical(aligned_phys.as_u64() as usize, aligned_size)?;

        // 映射
        let base = map_vmo(
            &vmo,
            0,
            aligned_size,
            MappingFlags::READ | MappingFlags::WRITE,
        )?;

        // 调整基地址以反映页内偏移
        let actual_base = base.add(page_offset);

        Ok(Self {
            vmo,
            base: actual_base,
            size,
            phys_addr,
        })
    }

    /// 获取基地址
    #[inline]
    pub fn base(&self) -> *mut u8 {
        self.base
    }

    /// 获取大小
    #[inline]
    pub fn size(&self) -> usize {
        self.size
    }

    /// 获取物理地址
    #[inline]
    pub fn phys_addr(&self) -> PhysAddr {
        self.phys_addr
    }

    /// 读取寄存器
    #[inline]
    pub fn read<T: Copy>(&self, offset: usize) -> T {
        assert!(offset + core::mem::size_of::<T>() <= self.size);
        unsafe { read_volatile((self.base as *const u8).add(offset) as *const T) }
    }

    /// 写入寄存器
    #[inline]
    pub fn write<T: Copy>(&self, offset: usize, value: T) {
        assert!(offset + core::mem::size_of::<T>() <= self.size);
        unsafe { write_volatile((self.base as *mut u8).add(offset) as *mut T, value) }
    }

    /// 读取 u8
    #[inline]
    pub fn read_u8(&self, offset: usize) -> u8 {
        self.read(offset)
    }

    /// 写入 u8
    #[inline]
    pub fn write_u8(&self, offset: usize, value: u8) {
        self.write(offset, value)
    }

    /// 读取 u16
    #[inline]
    pub fn read_u16(&self, offset: usize) -> u16 {
        self.read(offset)
    }

    /// 写入 u16
    #[inline]
    pub fn write_u16(&self, offset: usize, value: u16) {
        self.write(offset, value)
    }

    /// 读取 u32
    #[inline]
    pub fn read_u32(&self, offset: usize) -> u32 {
        self.read(offset)
    }

    /// 写入 u32
    #[inline]
    pub fn write_u32(&self, offset: usize, value: u32) {
        self.write(offset, value)
    }

    /// 读取 u64
    #[inline]
    pub fn read_u64(&self, offset: usize) -> u64 {
        self.read(offset)
    }

    /// 写入 u64
    #[inline]
    pub fn write_u64(&self, offset: usize, value: u64) {
        self.write(offset, value)
    }

    /// 获取寄存器引用
    pub fn reg<T: Copy>(&self, offset: usize) -> Register<T> {
        Register {
            ptr: unsafe { (self.base as *const u8).add(offset) as *mut T },
            _marker: PhantomData,
        }
    }

    /// 修改寄存器
    pub fn modify<T: Copy>(&self, offset: usize, f: impl FnOnce(T) -> T) {
        let val = self.read::<T>(offset);
        self.write(offset, f(val));
    }

    /// 设置位
    pub fn set_bits_u32(&self, offset: usize, bits: u32) {
        self.modify(offset, |v: u32| v | bits);
    }

    /// 清除位
    pub fn clear_bits_u32(&self, offset: usize, bits: u32) {
        self.modify(offset, |v: u32| v & !bits);
    }

    /// 等待位被设置
    pub fn wait_bits_set_u32(&self, offset: usize, bits: u32, timeout_us: u64) -> bool {
        // let start = 0u64; // TODO: 获取当前时间

        loop {
            if self.read_u32(offset) & bits == bits {
                return true;
            }

            // TODO: 检查超时
            // if current_time() - start > timeout_us {
            //     return false;
            // }

            core::hint::spin_loop();
        }
    }

    /// 等待位被清除
    pub fn wait_bits_clear_u32(&self, offset: usize, bits: u32, timeout_us: u64) -> bool {
        let start = 0u64;

        loop {
            if self.read_u32(offset) & bits == 0 {
                return true;
            }

            core::hint::spin_loop();
        }
    }
}

impl Drop for MmioRegion {
    fn drop(&mut self) {
        // VMO drop 时会自动 unmap
    }
}

unsafe impl Send for MmioRegion {}
unsafe impl Sync for MmioRegion {}

/// 寄存器引用
pub struct Register<T> {
    ptr: *mut T,
    _marker: PhantomData<T>,
}

impl<T: Copy> Register<T> {
    /// 读取
    #[inline]
    pub fn read(&self) -> T {
        unsafe { read_volatile(self.ptr) }
    }

    /// 写入
    #[inline]
    pub fn write(&self, value: T) {
        unsafe { write_volatile(self.ptr, value) }
    }

    /// 修改
    #[inline]
    pub fn modify(&self, f: impl FnOnce(T) -> T) {
        let val = self.read();
        self.write(f(val));
    }
}

unsafe impl<T> Send for Register<T> {}
unsafe impl<T> Sync for Register<T> {}

/// 定义寄存器偏移
#[macro_export]
macro_rules! define_regs {
    (
        $vis:vis struct $name:ident {
            $(
                $(#[$attr:meta])*
                $reg_name:ident : $reg_type:ty where $offset:expr
            ),* $(,)?
        }
    ) => {
        $vis struct $name {
            mmio: $crate::MmioRegion,
        }

        impl $name {
            pub fn new(mmio: $crate::MmioRegion) -> Self {
                Self { mmio }
            }

            $(
                $(#[$attr])*
                #[inline]
                pub fn $reg_name(&self) -> $crate::mmio::Register<$reg_type> {
                    self.mmio.reg($offset)
                }
            )*
        }
    };
}
