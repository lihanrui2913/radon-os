#![no_std]
#![no_main]
#![allow(unsafe_op_in_unsafe_fn)]
extern crate alloc;

use alloc::format;
use libdriver::{DriverClient, DriverOp};
use libradon::{error, info};
use pcid::protocol::{PciDeviceInfo, PciGetDeviceInfoRequest};
use radon_kernel::{ENOENT, EOPNOTSUPP, Error};

/// Ahci 进程主入口
#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    match libradon::init() {
        Ok(()) => match ahci_main() {
            Ok(()) => {
                libradon::process::exit(0);
            }
            Err(_) => {
                error!("ahci: main function have some problems");
                libradon::process::exit(-1)
            }
        },
        Err(_) => libradon::process::exit(-1),
    }
}

fn ahci_main() -> radon_kernel::Result<()> {
    let pci_service = DriverClient::connect("pci").map_err(|_| Error::new(ENOENT))?;
    let mut request = PciGetDeviceInfoRequest::default();
    request.class = 0x01;
    request.subclass = 0x06;
    request.interface = 0x01;
    let response = pci_service
        .call(DriverOp::Open, request.to_bytes())
        .map_err(|_| Error::new(EOPNOTSUPP))?;
    let pci_device_infos = unsafe {
        core::slice::from_raw_parts(
            response.data.as_ptr() as *const PciDeviceInfo,
            response.data.len() / size_of::<PciDeviceInfo>(),
        )
    }
    .to_vec();

    for (idx, pci_device_info) in pci_device_infos.iter().enumerate() {
        let name = format!("ahci{}", idx);
        info!(
            "{}: {}, bar5: {}",
            name, pci_device_info, pci_device_info.bars[5]
        );
    }

    Ok(())
}
