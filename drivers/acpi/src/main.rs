#![no_std]
#![no_main]
#![allow(unsafe_op_in_unsafe_fn)]

extern crate alloc;

mod acpi_table;

use acpi::sdt::mcfg::Mcfg;
use acpid::protocol::{self, AcpiMcfg};
use alloc::{string::String, vec::Vec};
use libdriver::{
    server::{ConnectionContext, RequestContext, RequestHandler},
    Request, Response, ServiceBuilder,
};
use libradon::{error, syscall::result_from_retval};
use radon_kernel::{Error, EINVAL};

use crate::acpi_table::Acpi;

/// Acpi 进程主入口
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

struct AcpiDriverHandler {
    acpi: Acpi,
}

impl RequestHandler for AcpiDriverHandler {
    fn handle(&self, request: &Request, _ctx: &RequestContext) -> Response {
        let table_header = String::from_utf8(request.data.clone()).unwrap();
        if table_header == "MCFG" {
            if let Some(mcfg) = self.acpi.table.find_table::<Mcfg>() {
                let mut entries = Vec::new();
                for entry in mcfg.entries() {
                    entries.push(AcpiMcfg {
                        base_address: entry.base_address,
                        segment_group: entry.pci_segment_group,
                        bus_start: entry.bus_number_start,
                        bus_end: entry.bus_number_end,
                    });
                }

                let mut acpi_mcfg = Vec::new();
                for entry in entries {
                    acpi_mcfg.extend_from_slice(entry.to_bytes());
                }

                Response::success(request.header.request_id).with_data(acpi_mcfg)
            } else {
                Response::error(
                    request.header.request_id,
                    protocol::ACPI_DAEMON_STATUS_NOT_FOUND,
                )
            }
        } else {
            Response::error(
                request.header.request_id,
                protocol::ACPI_DAEMON_STATUS_NOT_FOUND,
            )
        }
    }

    fn on_connect(&self, _ctx: &ConnectionContext) -> libdriver::Result<()> {
        Ok(())
    }

    fn on_disconnect(&self, _ctx: &ConnectionContext) {}
}

fn acpi_main() -> radon_kernel::Result<()> {
    let ret = unsafe { libradon::syscall0(libradon::syscall::nr::SYS_KRES_GET_RSDP) };
    let rsdp = result_from_retval(ret)?;

    let acpi_server = ServiceBuilder::new("acpi")
        .build(AcpiDriverHandler {
            acpi: Acpi::new(rsdp)?,
        })
        .map_err(|_| Error::new(EINVAL))?;

    acpi_server.run().map_err(|_| Error::new(EINVAL))?;

    Ok(())
}
