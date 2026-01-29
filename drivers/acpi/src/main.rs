#![no_std]
#![no_main]

use libradon::{error, info};

/// Init 进程主入口
#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    match libradon::init() {
        Ok(()) => match acpi_main() {
            Ok(()) => {
                libradon::process::exit(0);
            }
            Err(_) => {
                error!("acpi: main function have some problems");
                libradon::process::exit(-1)
            }
        },
        Err(_) => {
            // 日志错误
            libradon::process::exit(-1);
        }
    }
}

fn acpi_main() -> radon_kernel::Result<()> {
    info!("Acpi daemon starting...");
    Ok(())
}
