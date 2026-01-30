#![no_std]
#![no_main]
#![allow(unsafe_op_in_unsafe_fn)]

use alloc::{format, sync::Arc};
use libdriver::{
    DriverClient, DriverOp, PhysAddr, Request, RequestHandler, Response, ServiceBuilder,
    server::{ConnectionContext, RequestContext},
};
use libradon::{
    async_rt::{global_executor, spawn},
    error, info,
};
use pcid::protocol::{PciDeviceInfo, PciGetDeviceInfoRequest};
use radon_kernel::{EINVAL, ENOENT, EOPNOTSUPP, Error};

use crate::nvme::{NvmeController, NvmeNamespace};

extern crate alloc;

pub mod nvme;

/// Nvme 进程主入口
#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    match libradon::init() {
        Ok(()) => match nvme_main() {
            Ok(()) => {
                libradon::process::exit(0);
            }
            Err(_) => {
                error!("nvme: main function have some problems");
                libradon::process::exit(-1)
            }
        },
        Err(_) => {
            // 日志错误
            libradon::process::exit(-1);
        }
    }
}

struct NvmeDriverHandler(Arc<NvmeNamespace>);

impl RequestHandler for NvmeDriverHandler {
    fn handle(&self, request: &Request, _ctx: &RequestContext) -> Response {
        Response::error(request.header.request_id, 1)
    }

    fn on_connect(&self, _ctx: &ConnectionContext) -> libdriver::Result<()> {
        Ok(())
    }

    fn on_disconnect(&self, _ctx: &ConnectionContext) {}
}

async fn nvme_daemon(idx: usize, ns_idx: usize, ns: Arc<NvmeNamespace>) {
    let name = format!("nvme{}n{}", idx, ns_idx);

    let nvme_server = ServiceBuilder::new(&name)
        .build(NvmeDriverHandler(ns))
        .map_err(|_| Error::new(EINVAL))
        .expect("Failed to build service");

    nvme_server
        .run()
        .map_err(|_| Error::new(EINVAL))
        .expect("Failed to run service");
}

fn nvme_main() -> radon_kernel::Result<()> {
    let pci_service = DriverClient::connect("pci").map_err(|_| Error::new(ENOENT))?;
    let mut request = PciGetDeviceInfoRequest::default();
    request.class = 0x01;
    request.subclass = 0x08;
    request.interface = 0x02;
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
        let name = format!("nvme{}", idx);

        info!(
            "{}: {}, bar0: {}",
            name, pci_device_info, pci_device_info.bars[0]
        );

        let controller = unsafe {
            NvmeController::new(
                PhysAddr::new(pci_device_info.bars[0].address),
                pci_device_info.bars[0].size as usize,
            )
        }
        .expect("Failed to init nvme controller");

        // 先只扫描前4个
        (1..=4).for_each(|ns_idx| {
            if let Ok(ns) = controller.get_namespace(ns_idx as u32)
                && ns.info().capacity != 0
            {
                info!("Registering namespace {}", ns_idx);
                spawn(nvme_daemon(idx, ns_idx, ns));
            }
        });
    }

    global_executor()
        .expect("Failed to get global executor")
        .run();

    Ok(())
}
