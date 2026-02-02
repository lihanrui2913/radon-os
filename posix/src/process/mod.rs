use core::sync::atomic::AtomicUsize;

use alloc::{
    collections::btree_map::BTreeMap,
    string::String,
    sync::{Arc, Weak},
    vec,
    vec::Vec,
};
use libradon::{handle::Handle, memory::Vmo, process::Process};
use namespace::protocol::NAMESPACE_FILE_TYPE_REGULAR;
use radon_kernel::{EINVAL, ENOEXEC, Error, Result, layout};
use spin::{Mutex, RwLock};

use crate::{
    fs::open_inner,
    process::{
        program::{LoadedProgram, ProgramLoader},
        vma::VmArea,
    },
};

mod elf;
pub mod file;
mod program;
pub mod vma;

pub type VirtualAddress = usize;
pub type PhysicalAddress = usize;

pub struct PosixVmContext {
    vmar_handle: Handle,
    maps: BTreeMap<VirtualAddress, VmArea>,
}
pub struct PosixFsContext {}
pub struct PosixFileContext {}
pub struct PosixSignalContext {}

pub struct PosixProcess {
    pub pid: usize,
    pub name: String,
    pub path: String,
    pub process: Process,
    pub vm: PosixVmContext,
    pub fs: PosixFsContext,
    pub file: PosixFileContext,
    pub signal: PosixSignalContext,
}

pub type ArcPosixProcess = Arc<RwLock<PosixProcess>>;
pub type WeakPosixProcess = Weak<RwLock<PosixProcess>>;

pub static PROCESSES: Mutex<Vec<ArcPosixProcess>> = Mutex::new(Vec::new());

pub static NEXT_PID: AtomicUsize = AtomicUsize::new(1);

fn write_to_stack(stack_vmo: &Vmo, stack_top: usize, stack_point: usize, buf: &[u8]) -> Result<()> {
    stack_vmo
        .write(layout::STACK_TOP - (stack_top - stack_point), buf)
        .map(|_| ())
}

fn write_usize(stack_vmo: &Vmo, stack_top: usize, stack_point: usize, value: usize) -> Result<()> {
    write_to_stack(stack_vmo, stack_top, stack_point, &value.to_ne_bytes())
}

pub const AT_NULL: usize = 0;
pub const AT_IGNORE: usize = 1;
pub const AT_EXECFD: usize = 2;
pub const AT_PHDR: usize = 3;
pub const AT_PHENT: usize = 4;
pub const AT_PHNUM: usize = 5;
pub const AT_PAGESZ: usize = 6;
pub const AT_BASE: usize = 7;
pub const AT_FLAGS: usize = 8;
pub const AT_ENTRY: usize = 9;
pub const AT_UID: usize = 11;
pub const AT_EUID: usize = 12;
pub const AT_GID: usize = 13;
pub const AT_EGID: usize = 14;
pub const AT_PLATFORM: usize = 15;
pub const AT_HWCAP: usize = 16;
pub const AT_CLKTCK: usize = 17;
pub const AT_SECURE: usize = 23;
pub const AT_RANDOM: usize = 25;
pub const AT_HWCAP2: usize = 26;
pub const AT_EXECFN: usize = 31;

fn setup_user_stack(
    stack_vmo: &Vmo,
    stack_top: usize,
    argv: &[String],
    envp: &[String],
    elf_result: &LoadedProgram,
    interp_base: usize,
) -> Result<usize> {
    let mut sp = stack_top;

    let random_bytes = [0u8; 16];
    sp -= 16;
    let at_random_addr = sp;
    write_to_stack(stack_vmo, stack_top, sp, &random_bytes)?;

    #[cfg(target_arch = "x86_64")]
    let platform = b"x86_64\0";
    #[cfg(target_arch = "aarch64")]
    let platform = b"aarch64\0";
    #[cfg(target_arch = "riscv64")]
    let platform = b"riscv64\0";
    #[cfg(target_arch = "loongarch64")]
    let platform = b"loongarch64\0";

    sp -= platform.len();
    let platform_addr = sp;
    write_to_stack(stack_vmo, stack_top, sp, platform)?;

    let execfn = if !argv.is_empty() {
        argv[0].as_bytes()
    } else {
        b"unknown"
    };
    sp -= execfn.len() + 1;
    let execfn_addr = sp;
    write_to_stack(stack_vmo, stack_top, sp, execfn)?;
    write_to_stack(stack_vmo, stack_top, sp + execfn.len(), &[0u8])?;

    let mut envp_addrs = Vec::with_capacity(envp.len());
    for env in envp.iter().rev() {
        sp -= env.len() + 1;
        envp_addrs.push(sp);
        write_to_stack(stack_vmo, stack_top, sp, env.as_bytes())?;
        write_to_stack(stack_vmo, stack_top, sp + env.len(), &[0u8])?;
    }
    envp_addrs.reverse();

    let mut argv_addrs = Vec::with_capacity(argv.len());
    for arg in argv.iter().rev() {
        sp -= arg.len() + 1;
        argv_addrs.push(sp);
        write_to_stack(stack_vmo, stack_top, sp, arg.as_bytes())?;
        write_to_stack(stack_vmo, stack_top, sp + arg.len(), &[0u8])?;
    }
    argv_addrs.reverse();

    sp &= !0xF;

    let auxv: Vec<(usize, usize)> = vec![
        (AT_PHDR, elf_result.phdr_vaddr),
        (AT_PHENT, elf_result.phent_size),
        (AT_PHNUM, elf_result.phnum),
        (AT_PAGESZ, 4096),
        (AT_BASE, interp_base),
        (AT_ENTRY, elf_result.entry),
        (AT_UID, 0),
        (AT_EUID, 0),
        (AT_GID, 0),
        (AT_EGID, 0),
        (AT_SECURE, 0),
        (AT_RANDOM, at_random_addr),
        (AT_PLATFORM, platform_addr),
        (AT_EXECFN, execfn_addr),
        (AT_NULL, 0),
    ];

    let argc = argv.len();
    let auxv_size = auxv.len() * 2 * size_of::<usize>();
    let envp_ptr_size = (envp.len() + 1) * size_of::<usize>();
    let argv_ptr_size = (argc + 1) * size_of::<usize>();
    let argc_size = size_of::<usize>();
    let total_size = auxv_size + envp_ptr_size + argv_ptr_size + argc_size;

    sp -= total_size;
    sp &= !0xF;

    let stack_pointer = sp;

    write_usize(stack_vmo, stack_top, sp, argc)?;
    sp += size_of::<usize>();

    for &addr in &argv_addrs {
        write_usize(stack_vmo, stack_top, sp, addr)?;
        sp += size_of::<usize>();
    }
    write_usize(stack_vmo, stack_top, sp, 0)?;
    sp += size_of::<usize>();

    for &addr in &envp_addrs {
        write_usize(stack_vmo, stack_top, sp, addr)?;
        sp += size_of::<usize>();
    }
    write_usize(stack_vmo, stack_top, sp, 0)?;
    sp += size_of::<usize>();

    for (aux_type, aux_val) in auxv {
        write_usize(stack_vmo, stack_top, sp, aux_type)?;
        sp += size_of::<usize>();
        write_usize(stack_vmo, stack_top, sp, aux_val)?;
        sp += size_of::<usize>();
    }

    Ok(stack_pointer)
}

impl PosixProcess {
    pub fn new(path: String, argv: &[String], envp: &[String]) -> Result<ArcPosixProcess> {
        let (vmo, file_ty) = open_inner(path.clone())?;
        if file_ty != NAMESPACE_FILE_TYPE_REGULAR {
            return Err(Error::new(EINVAL));
        }
        let process = Process::create(&path.clone())?.bootstrap(true).build()?;

        let size = vmo.size()?;
        let mut buf = vec![0u8; size];
        vmo.read(0, &mut buf)?;

        let load_result = ProgramLoader::load(&process, &buf).map_err(|_| Error::new(ENOEXEC))?;

        let stack_top = setup_user_stack(
            &load_result.stack_vmo,
            load_result.stack_top,
            argv,
            envp,
            &load_result,
            load_result.interp_base,
        )?;

        process.create_thread(&path.clone(), load_result.entry, stack_top, 0)?;

        let vmar_handle = process.get_vmar_handle()?;
        let pid = NEXT_PID.fetch_add(1, core::sync::atomic::Ordering::SeqCst);
        let this = Arc::new(RwLock::new(Self {
            pid: pid,
            name: path.clone(),
            path: path.clone(),
            process: process,
            vm: PosixVmContext {
                vmar_handle,
                maps: BTreeMap::new(),
            },
            fs: PosixFsContext {},
            file: PosixFileContext {},
            signal: PosixSignalContext {},
        }));
        PROCESSES.lock().push(this.clone());
        Ok(this)
    }

    pub fn start(&self) -> Result<()> {
        self.process.start()
    }
}
