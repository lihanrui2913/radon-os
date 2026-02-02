#![no_std]
#![no_main]
#![allow(unsafe_op_in_unsafe_fn)]

use alloc::{format, string::String, sync::Arc, vec, vec::Vec};
use block_protocol::protocol::{BLOCK_ERR_IO, BlockDevice, PartitionDevice, probe_parititons};
use libdriver::{
    DriverClient, DriverOp, DriverServer, PhysAddr, Request, RequestHandler, Response,
    ServiceBuilder,
    protocol::IoRequest,
    server::{ConnectionContext, RequestContext},
};
use libradon::{debug, error, info};
use pcid::protocol::{PciDeviceInfo, PciGetDeviceInfoRequest};
use radon_kernel::{EINVAL, ENOENT, EOPNOTSUPP, Error, Result};
use spin::Mutex;

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
        Err(_) => libradon::process::exit(-1),
    }
}

#[derive(Clone)]
struct NvmeDriverHandler(Arc<NvmeNamespace>);

impl BlockDevice for NvmeDriverHandler {
    fn read_block(&self, start_byte: u64, buf: &mut [u8]) -> Result<()> {
        if buf.is_empty() {
            return Ok(());
        }

        let block_size = self.0.info().block_size as usize;

        let start = start_byte as usize;
        let end = start + buf.len();

        let start_block_id = start / block_size;
        let end_block_id = (end - 1) / block_size;

        let mut temp_block = vec![0u8; block_size];
        let mut buf_offset = 0;

        for block_id in start_block_id..=end_block_id {
            self.0.read_to_slice(block_id as u64, &mut temp_block)?;

            let block_start_byte = block_id * block_size;

            let offset_in_block = if block_id == start_block_id {
                start - block_start_byte
            } else {
                0
            };

            let end_in_block = if block_id == end_block_id {
                end - block_start_byte
            } else {
                block_size
            };

            let bytes_to_copy = end_in_block - offset_in_block;

            buf[buf_offset..buf_offset + bytes_to_copy]
                .copy_from_slice(&temp_block[offset_in_block..end_in_block]);

            buf_offset += bytes_to_copy;
        }

        Ok(())
    }

    fn write_block(&self, start_byte: u64, buf: &[u8]) -> Result<()> {
        if buf.is_empty() {
            return Ok(());
        }

        let block_size = self.0.info().block_size as usize;

        let start = start_byte as usize;
        let end = start + buf.len();

        let start_block_id = start / block_size;
        let end_block_id = (end - 1) / block_size;

        let mut temp_block = vec![0u8; block_size];
        let mut buf_offset = 0;

        for block_id in start_block_id..=end_block_id {
            let block_start_byte = block_id * block_size;

            let offset_in_block = if block_id == start_block_id {
                start - block_start_byte
            } else {
                0
            };

            let end_in_block = if block_id == end_block_id {
                end - block_start_byte
            } else {
                block_size
            };

            let bytes_to_copy = end_in_block - offset_in_block;

            if offset_in_block != 0 || end_in_block != block_size {
                self.0.read_to_slice(block_id as u64, &mut temp_block)?;
            }

            temp_block[offset_in_block..end_in_block]
                .copy_from_slice(&buf[buf_offset..buf_offset + bytes_to_copy]);

            self.0.write_from_slice(block_id as u64, &temp_block)?;

            buf_offset += bytes_to_copy;
        }

        Ok(())
    }

    fn size(&self) -> usize {
        self.0.info().capacity as usize
    }
}

impl RequestHandler for NvmeDriverHandler {
    fn handle(&self, request: &Request, _ctx: &RequestContext) -> Response {
        match DriverOp::from(request.header.op) {
            DriverOp::Read => {
                let io_request =
                    unsafe { (request.data.as_ptr() as *const IoRequest).as_ref() }.unwrap();
                let mut buf = Vec::with_capacity(io_request.length as usize);
                if let Err(_) = self.read_block(io_request.offset, &mut buf) {
                    Response::error(request.header.request_id, BLOCK_ERR_IO)
                } else {
                    Response::success(request.header.request_id).with_data(buf)
                }
            }
            DriverOp::Write => {
                let io_request =
                    unsafe { (request.data.as_ptr() as *const IoRequest).as_ref() }.unwrap();
                let buf = unsafe {
                    core::slice::from_raw_parts(
                        (request.data.as_ptr() as *const IoRequest).add(1) as *const u8,
                        io_request.length as usize,
                    )
                };
                if let Err(_) = self.write_block(io_request.offset, buf) {
                    Response::error(request.header.request_id, BLOCK_ERR_IO)
                } else {
                    Response::success(request.header.request_id)
                        .with_data((io_request.length).to_le_bytes().to_vec())
                }
            }
            // TODO: GetBuffer & ReleaseBuffer
            _ => Response::error(request.header.request_id, 1),
        }
    }

    fn on_connect(&self, _ctx: &ConnectionContext) -> libdriver::Result<()> {
        Ok(())
    }

    fn on_disconnect(&self, _ctx: &ConnectionContext) {}
}

pub static NVME_SERVICES: Mutex<Vec<DriverServer>> = Mutex::new(Vec::new());

fn nvme_register_partdev(name: String, part_dev: PartitionDevice) {
    info!("Registering partition {}", name);

    let part_server = ServiceBuilder::new(&name)
        .build(part_dev)
        .map_err(|_| Error::new(EINVAL))
        .expect("Failed to build service");

    NVME_SERVICES.lock().push(part_server);
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
                let name = format!("nvme{}n{}", idx, ns_idx);

                let block_dev = NvmeDriverHandler(ns);

                let nvme_server = ServiceBuilder::new(&name)
                    .build(block_dev.clone())
                    .map_err(|_| Error::new(EINVAL))
                    .expect("Failed to build service");

                NVME_SERVICES.lock().push(nvme_server);

                let _ = probe_parititons(&name, Arc::new(block_dev.clone()), nvme_register_partdev);
            }
        });
    }

    loop {
        for service in NVME_SERVICES.lock().iter() {
            service.run_once().map_err(|_| Error::new(EINVAL))?;
        }
    }
}
