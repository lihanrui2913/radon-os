#![no_std]
#![no_main]

extern crate alloc;

use bootstrap::{protocol::services, BootstrapClient};
use libradon::{error, info};
use nameserver::server::{Config, NameServer};

#[no_mangle]
pub extern "C" fn _start() -> ! {
    match libradon::init() {
        Ok(()) => match nameserver_main() {
            Ok(()) => libradon::process::exit(0),
            Err(_) => {
                error!("nameserver: main function have some problems");
                libradon::process::exit(-1)
            }
        },
        Err(_) => libradon::process::exit(-1),
    }
}

fn nameserver_main() -> Result<(), i32> {
    // 获取 bootstrap channel
    let bootstrap = BootstrapClient::connect().map_err(|_| -1)?;

    // 创建 Name Server
    let config = Config::default();
    let (server, ch) = NameServer::new(config).map_err(|_| -2)?;

    info!("Registering nameserver.");

    // 向 init 注册为 NAMESERVER 服务
    bootstrap
        .register_provider(services::NAMESERVER, &ch)
        .map_err(|_| -4)?;

    info!("Nameserver registered.");

    // 运行服务器
    server.run().map_err(|_| -5)?;

    Ok(())
}
