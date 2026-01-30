#![no_std]
#![no_main]

extern crate alloc;

pub mod elf;
pub mod program;

use core::sync::atomic::{AtomicBool, Ordering};

use libradon::{error, info, process::Process};

use bootstrap::BootstrapHandler;

use crate::program::ProgramLoader;

/// 全局运行标志
static RUNNING: AtomicBool = AtomicBool::new(true);

/// Init 进程主入口
#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    match libradon::init() {
        Ok(()) => match init_main() {
            Ok(()) => {
                libradon::process::exit(0);
            }
            Err(_) => {
                error!("init: main function have some problems");
                libradon::process::exit(-1)
            }
        },
        Err(_) => {
            // 日志错误
            libradon::process::exit(-1);
        }
    }
}

fn init_main() -> Result<(), InitError> {
    // 创建 bootstrap 处理器
    let bootstrap = BootstrapHandler::new().map_err(|_| InitError::BootstrapFailed)?;

    // 启动 Name Server
    start_nameserver(&bootstrap)?;
    info!("Nameserver started.");

    // 启动其他核心服务
    start_core_services(&bootstrap)?;
    info!("Core services started.");

    // 启动用户服务
    start_user_services(&bootstrap)?;
    info!("User services started.");

    // 运行事件循环
    run_event_loop(&bootstrap)?;

    Ok(())
}

static NAMESERVER_ELF: &'static [u8] = include_bytes!("../../nameserver/build/nameserver.elf");

/// 启动 Name Server
fn start_nameserver(bootstrap: &BootstrapHandler) -> Result<(), InitError> {
    // 创建 Name Server 进程
    let mut ns_process = Process::create("nameserver")
        .map_err(|_| InitError::ProcessFailed)?
        .bootstrap(true)
        .build()
        .map_err(|_| InitError::ProcessFailed)?;

    // 获取 Name Server 的 bootstrap channel
    let ns_bootstrap = ns_process
        .take_bootstrap()
        .ok_or(InitError::ProcessFailed)?;

    // 注册 Name Server 为我们的子进程（特权）
    let _child_id = bootstrap.add_child(ns_bootstrap, true);

    let loaded =
        ProgramLoader::load(&ns_process, NAMESERVER_ELF).map_err(|_| InitError::ProcessFailed)?;

    ns_process
        .create_thread("ns_main", loaded.entry, loaded.stack_top, 0)
        .map_err(|_| InitError::ProcessFailed)?;

    // 启动 Name Server
    ns_process.start().map_err(|_| InitError::ProcessFailed)?;

    while !bootstrap.ping_service(bootstrap::services::NAMESERVER) {
        bootstrap.poll().map_err(|_| InitError::BootstrapFailed)?;
    }

    Ok(())
}

static ACPI_ELF: &'static [u8] = include_bytes!("../../drivers/acpi/build/acpi.elf");
static PCI_ELF: &'static [u8] = include_bytes!("../../drivers/pci/build/pci.elf");
static NVME_ELF: &'static [u8] = include_bytes!("../../drivers/nvme/build/nvme.elf");

/// 启动核心服务
fn start_core_services(bootstrap: &BootstrapHandler) -> Result<(), InitError> {
    start_service(bootstrap, "acpi", ACPI_ELF, false)?;
    start_service(bootstrap, "pci", PCI_ELF, false)?;
    start_service(bootstrap, "nvme", NVME_ELF, false)?;
    Ok(())
}

/// 启动用户服务
fn start_user_services(_bootstrap: &BootstrapHandler) -> Result<(), InitError> {
    Ok(())
}

/// 启动一个服务进程
fn start_service(
    bootstrap: &BootstrapHandler,
    name: &str,
    buf: &[u8],
    privileged: bool,
) -> Result<Process, InitError> {
    // 创建 Name Server 进程
    let mut process = Process::create(name)
        .map_err(|_| InitError::ProcessFailed)?
        .bootstrap(true)
        .build()
        .map_err(|_| InitError::ProcessFailed)?;

    // 获取 bootstrap channel
    let process_bootstrap = process.take_bootstrap().ok_or(InitError::ProcessFailed)?;

    // 注册
    let _child_id = bootstrap.add_child(process_bootstrap, privileged);

    let loaded = ProgramLoader::load(&process, buf).map_err(|_| InitError::ProcessFailed)?;

    process
        .create_thread(name, loaded.entry, loaded.stack_top, 0)
        .map_err(|_| InitError::ProcessFailed)?;

    // 启动
    process.start().map_err(|_| InitError::ProcessFailed)?;

    Ok(process)
}

/// 运行事件循环
fn run_event_loop(bootstrap: &BootstrapHandler) -> Result<(), InitError> {
    while RUNNING.load(Ordering::Relaxed) {
        // 处理 bootstrap 请求
        bootstrap.poll().map_err(|_| InitError::BootstrapFailed)?;

        // 处理其他事件
        // ...

        // 让出 CPU
        libradon::process::yield_now();
    }

    Ok(())
}

/// Init 错误
#[derive(Debug)]
enum InitError {
    BootstrapFailed,
    ProcessFailed,
}
