#![no_std]
#![no_main]

extern crate alloc;

use alloc::string::ToString;
use libradon::error;

use crate::process::PosixProcess;

mod fs;
mod process;

/// posix 进程主入口
#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    match libradon::init() {
        Ok(()) => match posix_main() {
            Ok(()) => {
                libradon::process::exit(0);
            }
            Err(_) => {
                error!("posix: main function have some problems");
                libradon::process::exit(-1)
            }
        },
        Err(_) => libradon::process::exit(-1),
    }
}

fn posix_main() -> radon_kernel::Result<()> {
    while nameserver::client::lookup("driver.rootns").is_err() {
        libradon::syscall::nanosleep(100_000_000)?;
    }

    let _init_process =
        PosixProcess::new("/sbin/init".to_string(), &[], &[]).expect("Failed to start posix init");

    Ok(())
}
