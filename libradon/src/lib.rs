#![no_std]
#![allow(unsafe_op_in_unsafe_fn)]

use core::panic::PanicInfo;

use radon_kernel::{Result, memory::heap::HEAP_ALLOCATOR};

use crate::memory::{MappingFlags, Vmo, VmoOptions, map_vmo};

extern crate alloc;
extern crate log;

pub use self::arch::{syscall0, syscall1, syscall2, syscall3, syscall4, syscall5, syscall6};
pub use log::{debug, error, info, trace, warn};

mod arch;
pub mod channel;
pub mod handle;
pub mod logger;
pub mod memory;
pub mod port;
pub mod process;
pub mod signal;
pub mod syscall;

pub mod async_rt;

const HEAP_SIZE: usize = 16 * 1024 * 1024;

fn init_heap() -> Result<()> {
    let mut vmo = Vmo::create(HEAP_SIZE, VmoOptions::COMMIT)?;
    vmo.with_nodrop(true);
    let vaddr = map_vmo(&vmo, 0, HEAP_SIZE, MappingFlags::READ | MappingFlags::WRITE)?;
    unsafe { HEAP_ALLOCATOR.lock().init(vaddr, HEAP_SIZE) };
    Ok(())
}

fn init_logger() -> Result<()> {
    logger::init();
    Ok(())
}

pub fn init() -> Result<()> {
    init_heap()?;
    init_logger()?;
    async_rt::init()?;
    Ok(())
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    error!("User Panic: {}", info);
    crate::syscall::exit(-1)
}
