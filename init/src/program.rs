//! 程序加载器

use libradon::{
    memory::{MappingFlags, Vmo, VmoOptions, map_vmo_at_in_vmar},
    process::Process,
};
use radon_kernel::layout;

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
    pub entry: usize,
    /// 栈顶
    pub stack_top: usize,
}

/// 程序加载器
pub struct ProgramLoader;

impl ProgramLoader {
    /// 加载 ELF 程序
    pub fn load(process: &Process, elf_data: &[u8]) -> Result<LoadedProgram, LoaderError> {
        // 解析 ELF
        let elf = ElfParser::parse(elf_data)?;

        // 计算基地址（对于 PIE）
        let base_address = if elf.is_pie() {
            // PIE 可以加载到任意地址，这里使用默认基地址
            0x0000_0001_0000_0000
        } else {
            0
        };

        let vmar_handle = process
            .get_vmar_handle()
            .map_err(|_| LoaderError::OutOfMemory)?;

        // 加载所有段
        for segment in elf.load_segments() {
            let vaddr = base_address + segment.vaddr;
            let aligned_vaddr = vaddr & !4095usize;
            let offset = vaddr - aligned_vaddr;
            let memsz = segment.memsz;
            let size = ((vaddr + memsz + 4095) & !4095usize) - aligned_vaddr;

            let vmo =
                Vmo::create(size, VmoOptions::COMMIT).map_err(|_| LoaderError::OutOfMemory)?;
            vmo.write(offset, segment.data.unwrap())
                .map_err(|_| LoaderError::OutOfMemory)?;
            map_vmo_at_in_vmar(
                vmar_handle,
                &vmo,
                offset,
                size,
                MappingFlags::READ | MappingFlags::WRITE | MappingFlags::EXECUTE,
                aligned_vaddr as *mut u8,
            )
            .map_err(|_| LoaderError::OutOfMemory)?;
        }

        // 分配栈
        let aligned_size = layout::DEFAULT_STACK_SIZE;
        let stack_bottom = layout::STACK_TOP - aligned_size;
        let vmo =
            Vmo::create(aligned_size, VmoOptions::COMMIT).map_err(|_| LoaderError::OutOfMemory)?;
        map_vmo_at_in_vmar(
            vmar_handle,
            &vmo,
            0,
            aligned_size,
            MappingFlags::READ | MappingFlags::WRITE,
            stack_bottom as *mut u8,
        )
        .map_err(|_| LoaderError::OutOfMemory)?;

        // 计算入口点
        let entry = base_address + elf.entry_point() as usize;

        Ok(LoadedProgram {
            entry,
            stack_top: layout::STACK_TOP,
        })
    }
}
