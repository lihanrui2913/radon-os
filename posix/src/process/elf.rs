use core::{ffi::CStr, mem::size_of};

/// ELF 魔数
pub const ELF_MAGIC: [u8; 4] = [0x7f, b'E', b'L', b'F'];

/// ELF 类型
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElfClass {
    None = 0,
    Elf32 = 1,
    Elf64 = 2,
}

/// 数据编码
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElfData {
    None = 0,
    Lsb = 1, // 小端
    Msb = 2, // 大端
}

/// ELF 文件类型
#[repr(u16)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElfType {
    None = 0,
    Relocatable = 1,
    Executable = 2,
    SharedObject = 3,
    Core = 4,
}

impl From<u16> for ElfType {
    fn from(v: u16) -> Self {
        match v {
            1 => ElfType::Relocatable,
            2 => ElfType::Executable,
            3 => ElfType::SharedObject,
            4 => ElfType::Core,
            _ => ElfType::None,
        }
    }
}

/// 机器架构
#[repr(u16)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElfMachine {
    None = 0,
    X86_64 = 62,
    AArch64 = 183,
    RiscV = 243,
    LoongArch = 258,
}

impl From<u16> for ElfMachine {
    fn from(v: u16) -> Self {
        match v {
            62 => ElfMachine::X86_64,
            183 => ElfMachine::AArch64,
            243 => ElfMachine::RiscV,
            258 => ElfMachine::LoongArch,
            _ => ElfMachine::None,
        }
    }
}

/// 程序段类型
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegmentType {
    Null = 0,
    Load = 1,
    Dynamic = 2,
    Interp = 3,
    Note = 4,
    Shlib = 5,
    Phdr = 6,
    Tls = 7,
    GnuEhFrame = 0x6474e550,
    GnuStack = 0x6474e551,
    GnuRelro = 0x6474e552,
}

impl From<u32> for SegmentType {
    fn from(v: u32) -> Self {
        match v {
            0 => SegmentType::Null,
            1 => SegmentType::Load,
            2 => SegmentType::Dynamic,
            3 => SegmentType::Interp,
            4 => SegmentType::Note,
            5 => SegmentType::Shlib,
            6 => SegmentType::Phdr,
            7 => SegmentType::Tls,
            0x6474e550 => SegmentType::GnuEhFrame,
            0x6474e551 => SegmentType::GnuStack,
            0x6474e552 => SegmentType::GnuRelro,
            _ => SegmentType::Null,
        }
    }
}

bitflags::bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct SegmentFlags: u32 {
        const READ = 0x1;
        const WRITE = 0x2;
        const EXECUTE = 0x4;
    }
}

/// ELF64 文件头
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Elf64Header {
    /// 魔数
    pub magic: [u8; 4],
    /// ELF 类型 (32/64)
    pub class: u8,
    /// 数据编码 (大端/小端)
    pub data: u8,
    /// 版本
    pub version: u8,
    /// OS/ABI
    pub os_abi: u8,
    /// ABI 版本
    pub abi_version: u8,
    /// 填充
    pub padding: [u8; 7],
    /// 文件类型
    pub elf_type: u16,
    /// 机器架构
    pub machine: u16,
    /// ELF 版本
    pub elf_version: u32,
    /// 入口点地址
    pub entry: u64,
    /// 程序头表偏移
    pub phoff: u64,
    /// 节头表偏移
    pub shoff: u64,
    /// 处理器特定标志
    pub flags: u32,
    /// ELF 头大小
    pub ehsize: u16,
    /// 程序头表项大小
    pub phentsize: u16,
    /// 程序头表项数量
    pub phnum: u16,
    /// 节头表项大小
    pub shentsize: u16,
    /// 节头表项数量
    pub shnum: u16,
    /// 节名称字符串表索引
    pub shstrndx: u16,
}

impl Elf64Header {
    pub const SIZE: usize = size_of::<Self>();

    /// 验证 ELF 头
    pub fn validate(&self) -> Result<(), ElfError> {
        // 检查魔数
        if self.magic != ELF_MAGIC {
            return Err(ElfError::InvalidMagic);
        }

        // 检查类型 (必须是 64 位)
        if self.class != ElfClass::Elf64 as u8 {
            return Err(ElfError::Not64Bit);
        }

        // 检查字节序 (必须是小端)
        if self.data != ElfData::Lsb as u8 {
            return Err(ElfError::InvalidEndian);
        }

        // 检查文件类型
        let elf_type = ElfType::from(self.elf_type);
        if elf_type != ElfType::Executable && elf_type != ElfType::SharedObject {
            return Err(ElfError::NotExecutable);
        }

        // 检查机器架构
        #[cfg(target_arch = "x86_64")]
        if ElfMachine::from(self.machine) != ElfMachine::X86_64 {
            return Err(ElfError::WrongArchitecture);
        }

        #[cfg(target_arch = "aarch64")]
        if ElfMachine::from(self.machine) != ElfMachine::AArch64 {
            return Err(ElfError::WrongArchitecture);
        }

        #[cfg(target_arch = "riscv64")]
        if ElfMachine::from(self.machine) != ElfMachine::RiscV {
            return Err(ElfError::WrongArchitecture);
        }

        #[cfg(target_arch = "loongarch64")]
        if ElfMachine::from(self.machine) != ElfMachine::LoongArch {
            return Err(ElfError::WrongArchitecture);
        }

        Ok(())
    }

    /// 获取文件类型
    pub fn elf_type(&self) -> ElfType {
        ElfType::from(self.elf_type)
    }

    /// 获取机器架构
    pub fn machine(&self) -> ElfMachine {
        ElfMachine::from(self.machine)
    }
}

/// ELF64 程序头
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Elf64ProgramHeader {
    /// 段类型
    pub seg_type: u32,
    /// 段标志
    pub flags: u32,
    /// 文件偏移
    pub offset: u64,
    /// 虚拟地址
    pub vaddr: u64,
    /// 物理地址
    pub paddr: u64,
    /// 文件中的大小
    pub filesz: u64,
    /// 内存中的大小
    pub memsz: u64,
    /// 对齐
    pub align: u64,
}

impl Elf64ProgramHeader {
    pub const SIZE: usize = size_of::<Self>();

    /// 获取段类型
    pub fn seg_type(&self) -> SegmentType {
        SegmentType::from(self.seg_type)
    }

    /// 获取段标志
    pub fn flags(&self) -> SegmentFlags {
        SegmentFlags::from_bits_truncate(self.flags)
    }

    /// 是否可加载
    pub fn is_load(&self) -> bool {
        self.seg_type() == SegmentType::Load
    }
}

/// ELF64 节头
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Elf64SectionHeader {
    /// 节名称（字符串表偏移）
    pub name: u32,
    /// 节类型
    pub sh_type: u32,
    /// 节标志
    pub flags: u64,
    /// 虚拟地址
    pub addr: u64,
    /// 文件偏移
    pub offset: u64,
    /// 节大小
    pub size: u64,
    /// 关联节索引
    pub link: u32,
    /// 额外信息
    pub info: u32,
    /// 对齐
    pub addralign: u64,
    /// 条目大小（如果是表）
    pub entsize: u64,
}

/// ELF 解析错误
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElfError {
    /// 无效的魔数
    InvalidMagic,
    /// 不是 64 位 ELF
    Not64Bit,
    /// 无效的字节序
    InvalidEndian,
    /// 不是可执行文件
    NotExecutable,
    /// 架构不匹配
    WrongArchitecture,
    /// 文件太小
    FileTooSmall,
    /// 无效的程序头
    InvalidProgramHeader,
    /// 段超出范围
    SegmentOutOfBounds,
}

/// ELF 文件解析器
pub struct ElfParser<'a> {
    data: &'a [u8],
    header: &'a Elf64Header,
}

impl<'a> ElfParser<'a> {
    /// 解析 ELF 文件
    pub fn parse(data: &'a [u8]) -> Result<Self, ElfError> {
        if data.len() < Elf64Header::SIZE {
            return Err(ElfError::FileTooSmall);
        }

        let header = unsafe { &*(data.as_ptr() as *const Elf64Header) };
        header.validate()?;

        Ok(Self { data, header })
    }

    /// 获取 ELF 头
    pub fn header(&self) -> &Elf64Header {
        self.header
    }

    /// 获取入口点
    pub fn entry_point(&self) -> u64 {
        self.header.entry
    }

    /// 获取程序头迭代器
    pub fn program_headers(&self) -> ProgramHeaderIter<'a> {
        ProgramHeaderIter {
            data: self.data,
            offset: self.header.phoff as usize,
            entry_size: self.header.phentsize as usize,
            count: self.header.phnum as usize,
            index: 0,
        }
    }

    /// 获取可加载段迭代器
    pub fn load_segments(&'a self) -> impl Iterator<Item = LoadSegment<'a>> + 'a {
        self.program_headers()
            .filter(|ph| ph.is_load())
            .map(move |ph| LoadSegment {
                vaddr: ph.vaddr as usize,
                memsz: ph.memsz as usize,
                filesz: ph.filesz as usize,
                offset: ph.offset as usize,
                flags: ph.flags(),
                data: if ph.filesz > 0 {
                    let start = ph.offset as usize;
                    let end = start + ph.filesz as usize;
                    if end <= self.data.len() {
                        Some(&self.data[start..end])
                    } else {
                        None
                    }
                } else {
                    None
                },
            })
    }

    pub fn phdr_segments(&'a self) -> Option<Elf64ProgramHeader> {
        self.program_headers()
            .find(|s| s.seg_type() == SegmentType::Phdr)
            .map(|s| s.clone())
    }

    pub fn interpreter(&'a self) -> Option<&'a str> {
        self.program_headers()
            .find(|s| s.seg_type() == SegmentType::Interp)
            .map(|s| (s.offset, s.filesz))
            .map(|(offset, len)| {
                unsafe {
                    CStr::from_bytes_with_nul_unchecked(
                        &self.data[offset as usize..(offset as usize + len as usize)],
                    )
                }
                .to_str()
                .unwrap_or("")
            })
    }

    /// 计算内存布局（需要的地址范围）
    pub fn memory_bounds(&self) -> Option<(usize, usize)> {
        let mut min_addr: Option<usize> = None;
        let mut max_addr: Option<usize> = None;

        for ph in self.program_headers() {
            if !ph.is_load() {
                continue;
            }

            let start = ph.vaddr as usize;
            let end = start + ph.memsz as usize;

            min_addr = Some(min_addr.map_or(start, |m| m.min(start)));
            max_addr = Some(max_addr.map_or(end, |m| m.max(end)));
        }

        match (min_addr, max_addr) {
            (Some(min), Some(max)) => Some((min, max)),
            _ => None,
        }
    }

    /// 是否为 PIE (位置无关可执行文件)
    pub fn is_pie(&self) -> bool {
        self.header.elf_type() == ElfType::SharedObject
    }
}

/// 程序头迭代器
pub struct ProgramHeaderIter<'a> {
    data: &'a [u8],
    offset: usize,
    entry_size: usize,
    count: usize,
    index: usize,
}

impl<'a> Iterator for ProgramHeaderIter<'a> {
    type Item = &'a Elf64ProgramHeader;

    fn next(&mut self) -> Option<Self::Item> {
        if self.index >= self.count {
            return None;
        }

        let ph_offset = self.offset + self.index * self.entry_size;
        if ph_offset + Elf64ProgramHeader::SIZE > self.data.len() {
            return None;
        }

        let ph = unsafe { &*(self.data.as_ptr().add(ph_offset) as *const Elf64ProgramHeader) };
        self.index += 1;
        Some(ph)
    }
}

/// 可加载段
#[derive(Debug)]
pub struct LoadSegment<'a> {
    /// 虚拟地址
    pub vaddr: usize,
    /// 内存大小
    pub memsz: usize,
    /// 文件大小
    pub filesz: usize,
    /// 文件偏移
    pub offset: usize,
    /// 权限标志
    pub flags: SegmentFlags,
    /// 段数据
    pub data: Option<&'a [u8]>,
}

impl<'a> LoadSegment<'a> {
    /// 是否可读
    pub fn is_readable(&self) -> bool {
        self.flags.contains(SegmentFlags::READ)
    }

    /// 是否可写
    pub fn is_writable(&self) -> bool {
        self.flags.contains(SegmentFlags::WRITE)
    }

    /// 是否可执行
    pub fn is_executable(&self) -> bool {
        self.flags.contains(SegmentFlags::EXECUTE)
    }
}
