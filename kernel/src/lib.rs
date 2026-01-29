#![no_std]
#![no_main]
#![allow(clippy::field_reassign_with_default)]
#![allow(clippy::missing_safety_doc)]
#![allow(clippy::mut_from_ref)]
#![allow(clippy::new_ret_no_self)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]
#![allow(clippy::too_many_arguments)]
#![allow(unsafe_op_in_unsafe_fn)]
#![feature(allocator_api)]
#![feature(int_roundings)]
#![feature(ptr_as_ref_unchecked)]
#![feature(sync_unsafe_cell)]

#[macro_use]
extern crate alloc;
#[macro_use]
extern crate log;

use core::hint::spin_loop;

use limine::{
    BaseRevision,
    request::{ModuleRequest, RequestsEndMarker, RequestsStartMarker, StackSizeRequest},
};

#[used]
#[unsafe(link_section = ".requests")]
static BASE_REVISION: BaseRevision = BaseRevision::with_revision(3);

#[used]
#[unsafe(link_section = ".requests")]
static STACK_SIZE_REQUEST: StackSizeRequest =
    StackSizeRequest::new().with_size(consts::STACK_SIZE as u64);

#[used]
#[unsafe(link_section = ".requests")]
static MODULE_REQUEST: ModuleRequest = ModuleRequest::new();

#[used]
#[unsafe(link_section = ".requests_start_marker")]
static _START_MARKER: RequestsStartMarker = RequestsStartMarker::new();
#[used]
#[unsafe(link_section = ".requests_end_marker")]
static _END_MARKER: RequestsEndMarker = RequestsEndMarker::new();

pub mod arch;
pub mod consts;
pub mod drivers;
pub mod init;
pub mod loader;
pub mod memory;
pub mod object;
pub mod smp;
pub mod syscall;
pub mod task;

pub use self::object::layout;
pub use self::syscall::error::*;
pub use self::syscall::nr;

use crate::arch::{CurrentIrqArch, irq::IrqArch};

macro_rules! linker_offsets(
    ($($name:ident),*) => {
        $(
        #[inline]
        pub fn $name() -> usize {
            unsafe extern "C" {
                static $name: u8;
            }
            (&raw const $name) as usize
        }
        )*
    }
);
mod kernel_executable_offsets {
    linker_offsets!(__start, __end);
    linker_offsets!(__text_start, __text_end, __rodata_start, __rodata_end);
}

#[unsafe(no_mangle)]
extern "C" fn kmain() -> ! {
    CurrentIrqArch::disable_global_irq();

    memory::heap::init();

    drivers::framebuffer::init();

    drivers::logger::init();

    arch::early_init();

    task::init().expect("Failed to execute kernel init");

    info!("Kernel initialized");

    loop {
        CurrentIrqArch::enable_global_irq();
        spin_loop();
    }
}

extern "C" fn initial_kernel_thread() -> ! {
    info!("Initial kernel thread is running");

    let initramfs_mod = MODULE_REQUEST.get_response().unwrap().modules()[0];
    let initramfs = unsafe {
        core::slice::from_raw_parts(
            initramfs_mod.addr() as *const u8,
            initramfs_mod.size() as usize,
        )
    };

    info!("Initramfs size: {} bytes", initramfs.len());

    // 查找 init 程序
    let mut init_found = false;
    for entry in cpio_reader::iter_files(initramfs) {
        let name = entry.name();

        if name.contains("init") {
            let elf_buf: &[u8] = entry.file();
            info!("Found init program, size: {} bytes", elf_buf.len());

            match load_and_run_init(elf_buf) {
                Ok(()) => {
                    init_found = true;
                    info!("Init process started successfully");
                    break;
                }
                Err(e) => {
                    error!("Failed to load init: {:?}", e);
                }
            }
        }
    }

    if !init_found {
        panic!("Init program not found in initramfs!");
    }

    // 进入调度循环
    loop {
        CurrentIrqArch::enable_global_irq();
        task::schedule();
    }
}

fn load_and_run_init(elf_data: &[u8]) -> Result<(), loader::LoaderError> {
    use loader::ProgramLoader;

    // 创建和启动进程
    let process = ProgramLoader::load_and_create_process(elf_data, "init")?;
    {
        let mut proc = process.write();
        proc.start();
    }

    info!("Init process created with PID: {}", process.read().pid());

    Ok(())
}
