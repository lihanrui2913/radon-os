//! 程序加载器

use alloc::{collections::btree_map::BTreeMap, sync::Arc};
use rmm::{Arch, FrameAllocator, PhysicalAddress, VirtualAddress};
use spin::Mutex;

use crate::{
    arch::CurrentRmmArch,
    init::memory::{FRAME_ALLOCATOR, PAGE_SIZE, align_down, align_up},
    object::{
        process::{ArcProcess, Process, layout, register_process},
        vmar::{MappingFlags, Vmar},
        vmo::{Vmo, VmoOptions},
    },
};

use super::elf::{ElfError, ElfParser};

/// 加载器错误
#[derive(Debug)]
pub enum LoaderError {
    /// ELF 解析错误
    ElfError(ElfError),
    /// 无效的程序
    InvalidProgram,
    /// 内存不足
    OutOfMemory,
}

impl From<ElfError> for LoaderError {
    fn from(e: ElfError) -> Self {
        LoaderError::ElfError(e)
    }
}

/// 加载的程序信息
pub struct LoadedProgram {
    /// 入口点
    pub entry: VirtualAddress,
    /// 栈顶
    pub stack_top: VirtualAddress,
    /// 地址空间
    pub root_vmar: Arc<Vmar>,
    /// 程序基地址（对于 PIE）
    pub base_address: VirtualAddress,
    /// BRK 地址（堆起始）
    pub brk: VirtualAddress,
}

pub static LOADED_PROGRAMS: Mutex<BTreeMap<usize, LoadedProgram>> = Mutex::new(BTreeMap::new());

/// 程序加载器
pub struct ProgramLoader;

impl ProgramLoader {
    /// 复制内核页表映射
    pub unsafe fn copy_kernel_mappings(page_table: PhysicalAddress) -> Result<(), LoaderError> {
        #[cfg(target_arch = "x86_64")]
        {
            use rmm::Arch;

            use crate::init::memory::KERNEL_PAGE_TABLE_PHYS;
            use core::sync::atomic::Ordering;

            let kernel_pml4 = KERNEL_PAGE_TABLE_PHYS.load(Ordering::SeqCst);
            let kernel_pml4_virt = CurrentRmmArch::phys_to_virt(PhysicalAddress::new(kernel_pml4));
            let new_pml4_virt = CurrentRmmArch::phys_to_virt(page_table);

            // 复制高半部分（内核空间）的 PML4 条目
            let src = kernel_pml4_virt.data() as *const u64;
            let dst = new_pml4_virt.data() as *mut u64;

            // PML4 有 512 个条目，高 256 个是内核空间
            for i in 256..512 {
                *dst.add(i) = *src.add(i);
            }
        }

        #[cfg(target_arch = "aarch64")]
        {
            // AArch64 使用 TTBR0 和 TTBR1 分开管理用户和内核空间
            // 不需要复制内核映射
        }

        #[cfg(target_arch = "riscv64")]
        {
            use crate::init::memory::KERNEL_PAGE_TABLE_PHYS;
            use core::sync::atomic::Ordering;

            let kernel_pt = KERNEL_PAGE_TABLE_PHYS.load(Ordering::SeqCst);
            let kernel_pt_virt = CurrentRmmArch::phys_to_virt(PhysicalAddress::new(kernel_pt));
            let new_pt_virt = CurrentRmmArch::phys_to_virt(page_table);

            // 复制高半部分
            let src = kernel_pt_virt.data() as *const u64;
            let dst = new_pt_virt.data() as *mut u64;

            for i in 256..512 {
                *dst.add(i) = *src.add(i);
            }
        }

        Ok(())
    }

    /// 加载 ELF 程序
    pub fn load(elf_data: &[u8], _name: &str) -> Result<LoadedProgram, LoaderError> {
        // 解析 ELF
        let elf = ElfParser::parse(elf_data)?;

        // 计算基地址（对于 PIE）
        let base_address = if elf.is_pie() {
            // PIE 可以加载到任意地址，这里使用默认基地址
            VirtualAddress::new(0x0000_0001_0000_0000)
        } else {
            VirtualAddress::new(0)
        };

        let mut max_end = 0usize;
        for segment in elf.load_segments() {
            let vaddr = base_address.data() + segment.vaddr;
            let memsz = segment.memsz;
            // 更新最大地址
            max_end = align_up(max_end.max(vaddr + memsz));
        }

        let new_page_table =
            unsafe { FRAME_ALLOCATOR.lock().allocate_one() }.ok_or(LoaderError::OutOfMemory)?;

        let new_page_table_virt = unsafe { CurrentRmmArch::phys_to_virt(new_page_table) };
        unsafe { core::ptr::write_bytes(new_page_table_virt.data() as *mut u8, 0, PAGE_SIZE) };

        unsafe { ProgramLoader::copy_kernel_mappings(new_page_table) }?;

        let user_base = VirtualAddress::new(layout::USER_SPACE_START);
        let user_size = layout::USER_SPACE_END - layout::USER_SPACE_START;
        let vmar = Vmar::create_root(user_base, user_size, layout::ALLOC_START, new_page_table);

        // 加载所有段
        for segment in elf.load_segments() {
            let vaddr = base_address.data() + segment.vaddr;
            let aligned_vaddr = align_down(vaddr);
            let offset = vaddr - aligned_vaddr;
            let memsz = segment.memsz;
            let size = align_up(vaddr + memsz) - aligned_vaddr;

            let vmo =
                Vmo::create(size, VmoOptions::COMMIT).map_err(|_| LoaderError::OutOfMemory)?;
            vmo.write(offset, segment.data.unwrap())
                .map_err(|_| LoaderError::OutOfMemory)?;
            vmar.map(
                vmo,
                0,
                size,
                MappingFlags::READ
                    | MappingFlags::WRITE
                    | MappingFlags::EXECUTE
                    | MappingFlags::SPECIFIC,
                Some(VirtualAddress::new(aligned_vaddr)),
            )
            .map_err(|_| LoaderError::OutOfMemory)?;
        }

        // 分配栈
        let aligned_size = layout::DEFAULT_STACK_SIZE;
        let stack_bottom = layout::STACK_TOP - aligned_size;

        let vmo =
            Vmo::create(aligned_size, VmoOptions::COMMIT).map_err(|_| LoaderError::OutOfMemory)?;
        vmar.map(
            vmo,
            0,
            aligned_size,
            MappingFlags::READ | MappingFlags::WRITE | MappingFlags::SPECIFIC,
            Some(VirtualAddress::new(stack_bottom)),
        )
        .map_err(|_| LoaderError::OutOfMemory)?;

        // 计算 BRK（堆起始地址）
        let brk = VirtualAddress::new((max_end + 0xFFF) & !0xFFF);

        // 计算入口点
        let entry = VirtualAddress::new(base_address.data() + elf.entry_point() as usize);

        Ok(LoadedProgram {
            entry,
            stack_top: VirtualAddress::new(layout::STACK_TOP),
            root_vmar: vmar,
            base_address,
            brk,
        })
    }

    /// 加载并创建进程
    pub fn load_and_create_process(elf_data: &[u8], name: &str) -> Result<ArcProcess, LoaderError> {
        // 加载程序
        let loaded = Self::load(elf_data, name)?;

        // 创建进程
        let process = Process::new(name.into(), None);

        // 设置地址空间
        {
            let mut proc = process.write();
            proc.set_root_vmar(loaded.root_vmar.clone());
            proc.set_brk(loaded.brk);
        }

        // 创建主线程
        let _main_thread = {
            let mut proc = process.write();

            proc.create_main_thread(loaded.entry.data(), loaded.stack_top.data() & !0xFusize)
        }
        .ok_or(LoaderError::OutOfMemory)?;

        LOADED_PROGRAMS.lock().insert(process.read().pid(), loaded);

        // 注册进程
        register_process(process.clone());

        Ok(process)
    }
}
