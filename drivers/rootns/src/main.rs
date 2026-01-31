#![no_std]
#![no_main]
#![allow(unsafe_op_in_unsafe_fn)]

use libradon::error;

extern crate alloc;

/// Rootns 进程主入口
#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    match libradon::init() {
        Ok(()) => match rootns_main() {
            Ok(()) => {
                libradon::process::exit(0);
            }
            Err(_) => {
                error!("rootns: main function have some problems");
                libradon::process::exit(-1)
            }
        },
        Err(_) => libradon::process::exit(-1),
    }
}

fn rootns_main() -> radon_kernel::Result<()> {
    Ok(())
}
