#![no_std]
#![no_main]
#![allow(unsafe_op_in_unsafe_fn)]

extern crate alloc;

use core::fmt::Display;

use acpid::protocol::AcpiMcfg;
use alloc::{string::String, vec::Vec};
use libdriver::{
    DriverClient, DriverOp, Request, Response, ServiceBuilder,
    server::{ConnectionContext, RequestContext, RequestHandler},
};
use libradon::{
    debug, error, info,
    memory::{MappingFlags, Vmo, map_vmo},
};
use pci_types::{
    Bar, BaseClass, CommandRegister, ConfigRegionAccess, DeviceId, DeviceRevision, EndpointHeader,
    HeaderType, Interface, MAX_BARS, PciAddress, PciHeader, PciPciBridgeHeader, SubClass,
    SubsystemId, SubsystemVendorId, VendorId, device_type::DeviceType,
};
use pcid::protocol::{
    BAR_TYPE_IO, BAR_TYPE_MMIO, BarInfo, PCI_STATUS_NOT_FOUND, PciDeviceInfo,
    PciGetDeviceInfoRequest,
};
use radon_kernel::{EINVAL, ENOENT, Error};
use spin::Mutex;

/// Pci 进程主入口
#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    match libradon::init() {
        Ok(()) => match pci_main() {
            Ok(()) => {
                libradon::process::exit(0);
            }
            Err(_) => {
                error!("pci: main function have some problems");
                libradon::process::exit(-1)
            }
        },
        Err(_) => {
            // 日志错误
            libradon::process::exit(-1);
        }
    }
}

#[derive(Debug, Clone)]
pub struct PciDevice {
    pub address: PciAddress,
    pub class: BaseClass,
    pub sub_class: SubClass,
    pub interface: Interface,
    pub vendor_id: VendorId,
    pub device_id: DeviceId,
    pub subsystem_vendor_id: SubsystemVendorId,
    pub subsystem_device_id: SubsystemId,
    pub revision: DeviceRevision,
    pub device_type: DeviceType,
    pub bars: [Option<Bar>; MAX_BARS],
}

impl Display for PciDevice {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        write!(
            f,
            "{}:{}.{}: {:?} [{:04x}:{:04x}] [{:04x}:{:04x}] (rev: {:02x})",
            self.address.bus(),
            self.address.device(),
            self.address.function(),
            self.device_type,
            self.vendor_id,
            self.device_id,
            self.subsystem_vendor_id,
            self.subsystem_device_id,
            self.revision,
        )
    }
}

pub static PCI_DEVICES: Mutex<Vec<PciDevice>> = Mutex::new(Vec::new());

fn find_pci_device_by_class_code(class: u8, subclass: u8, interface: u8) -> Vec<PciDevice> {
    PCI_DEVICES
        .lock()
        .iter()
        .filter(|device| {
            device.class == class && device.sub_class == subclass && device.interface == interface
        })
        .cloned()
        .collect::<Vec<_>>()
}

fn find_pci_device_by_vendor_device(vendor_id: u16, device_id: u16) -> Vec<PciDevice> {
    PCI_DEVICES
        .lock()
        .iter()
        .filter(|device| device.vendor_id == vendor_id && device.device_id == device_id)
        .cloned()
        .collect::<Vec<_>>()
}

struct PciDriverHandler;

impl RequestHandler for PciDriverHandler {
    fn handle(&self, request: &Request, _ctx: &RequestContext) -> Response {
        match DriverOp::from(request.header.op) {
            DriverOp::Open => {
                let get_device_request = PciGetDeviceInfoRequest::from_bytes(&request.data);
                let mut devices = Vec::new();
                let pci_devices_by_class_code = find_pci_device_by_class_code(
                    get_device_request.class,
                    get_device_request.subclass,
                    get_device_request.interface,
                );
                if pci_devices_by_class_code.is_empty() {
                    devices.extend_from_slice(&find_pci_device_by_vendor_device(
                        get_device_request.vendor,
                        get_device_request.device,
                    ));
                } else {
                    devices.extend_from_slice(&pci_devices_by_class_code);
                }
                let mut result = Vec::new();
                for device in devices {
                    let mut device_info = PciDeviceInfo {
                        bars: [BarInfo::default(); 6],
                        class: device.class,
                        subclass: device.sub_class,
                        interface: device.interface,
                        vendor: device.vendor_id,
                        device: device.device_id,
                        subsystem_vendor: device.subsystem_vendor_id,
                        subsystem_device: device.subsystem_device_id,
                        revision: device.revision,
                    };
                    for (idx, bar) in device.bars.iter().enumerate() {
                        if let Some(bar) = bar {
                            if let Bar::Io { port } = *bar {
                                device_info.bars[idx] = BarInfo {
                                    address: port as u64,
                                    size: 0,
                                    bar_type: BAR_TYPE_IO,
                                };
                            } else {
                                let (address, size) = bar.unwrap_mem();
                                device_info.bars[idx] = BarInfo {
                                    address: address as u64,
                                    size: size as u32,
                                    bar_type: BAR_TYPE_MMIO,
                                };
                            }
                        }
                    }
                    result.push(device_info);
                }

                let data = unsafe {
                    core::slice::from_raw_parts(
                        result.as_ptr() as *const u8,
                        result.len() * size_of::<PciDeviceInfo>(),
                    )
                }
                .to_vec();
                Response::success(request.header.request_id).with_data(data)
            }
            _ => Response::error(request.header.request_id, PCI_STATUS_NOT_FOUND),
        }
    }

    fn on_connect(&self, _ctx: &ConnectionContext) -> libdriver::Result<()> {
        Ok(())
    }

    fn on_disconnect(&self, _ctx: &ConnectionContext) {}
}

struct AcpiMcfgEntry {
    mcfg_entry: AcpiMcfg,
    base_vaddr: usize,
}

impl AcpiMcfgEntry {
    pub fn new(mcfg_entry: AcpiMcfg, base_vaddr: usize) -> Self {
        Self {
            mcfg_entry,
            base_vaddr,
        }
    }
}

struct PciAccess {
    entries: Vec<AcpiMcfgEntry>,
}

impl PciAccess {
    pub fn new(entries: Vec<AcpiMcfgEntry>) -> Self {
        Self { entries }
    }

    pub fn mmio_address(&self, address: PciAddress) -> Option<usize> {
        let (segment, bus, device, function) = (
            address.segment(),
            address.bus(),
            address.device(),
            address.function(),
        );

        let entry = self.entries.iter().find(|region| {
            region.mcfg_entry.segment_group == segment
                && (region.mcfg_entry.bus_start..=region.mcfg_entry.bus_end).contains(&bus)
        })?;

        Some(
            entry.base_vaddr
                + ((usize::from(bus - entry.mcfg_entry.bus_start) << 20)
                    | (usize::from(device) << 15)
                    | (usize::from(function) << 12)),
        )
    }
}

impl ConfigRegionAccess for PciAccess {
    unsafe fn read(&self, address: PciAddress, offset: u16) -> u32 {
        let mmio = self.mmio_address(address).unwrap() + offset as usize;
        core::ptr::read_volatile(mmio as *const u32)
    }

    unsafe fn write(&self, address: PciAddress, offset: u16, value: u32) {
        let mmio = self.mmio_address(address).unwrap() + offset as usize;
        core::ptr::write_volatile(mmio as *mut u32, value);
    }
}

fn pci_scan_function(segment_group: u16, bus: u8, device: u8, function: u8, access: &PciAccess) {
    let address = PciAddress::new(segment_group, bus, device, function);
    let header = PciHeader::new(address);

    let (vendor_id, device_id) = header.id(access);
    let (revision, class, sub_class, interface) = header.revision_and_class(access);

    if vendor_id == 0xffff {
        return;
    }

    let endpoint_bars = |header: &EndpointHeader| {
        let mut bars = [None; 6];
        let mut skip_next = false;

        for (index, bar_slot) in bars.iter_mut().enumerate() {
            if skip_next {
                skip_next = false;
                continue;
            }
            let bar = header.bar(index as u8, access);
            if let Some(Bar::Memory64 { .. }) = bar {
                skip_next = true;
            }
            *bar_slot = bar;
        }

        bars
    };

    match header.header_type(access) {
        HeaderType::Endpoint => {
            let mut endpoint_header =
                EndpointHeader::from_header(header, access).expect("Invalid endpoint header");

            let (subsystem_vendor_id, subsystem_device_id) = endpoint_header.subsystem(access);

            let bars = endpoint_bars(&endpoint_header);
            let device_type = DeviceType::from((class, sub_class));

            endpoint_header.update_command(access, |command| {
                command
                    | CommandRegister::BUS_MASTER_ENABLE
                    | CommandRegister::IO_ENABLE
                    | CommandRegister::MEMORY_ENABLE
            });

            let device = PciDevice {
                address,
                class,
                sub_class,
                interface,
                vendor_id,
                device_id,
                subsystem_vendor_id,
                subsystem_device_id,
                device_type,
                revision,
                bars,
            };

            PCI_DEVICES.lock().push(device);
        }
        HeaderType::PciPciBridge => {
            let bridge_header = PciPciBridgeHeader::from_header(header, access)
                .expect("Invalid PCI-PCI bridge header");

            let start_bus = bridge_header.secondary_bus_number(access);
            let end_bus = bridge_header.subordinate_bus_number(access);
            (start_bus..=end_bus).for_each(|bus_id| pci_scan_bus(segment_group, bus_id, access));
        }
        _ => {}
    }
}

fn pci_scan_bus(segment_group: u16, bus: u8, access: &PciAccess) {
    (0..32).for_each(|device| {
        let address = PciAddress::new(segment_group, bus, device, 0);
        pci_scan_function(segment_group, bus, device, 0, access);

        if PciHeader::new(address).has_multiple_functions(access) {
            (1..8).for_each(|function| {
                pci_scan_function(segment_group, bus, device, function, access)
            });
        }
    });
}

fn pci_main() -> radon_kernel::Result<()> {
    let acpi_service = DriverClient::connect("acpi").map_err(|_| Error::new(ENOENT))?;
    let mcfg_response = acpi_service
        .call(libdriver::DriverOp::Open, "MCFG".as_bytes())
        .map_err(|_| Error::new(ENOENT))?;
    let mcfg_entries_count = mcfg_response.data.len() / size_of::<AcpiMcfg>();
    let mcfg_entries = unsafe {
        core::slice::from_raw_parts(
            mcfg_response.data.as_ptr() as *const AcpiMcfg,
            mcfg_entries_count,
        )
    };

    let mut entries = Vec::new();

    for mcfg_entry in mcfg_entries {
        let region_base_addr = mcfg_entry.base_address;
        let aligned_region_base_addr = region_base_addr & !4095u64;
        let bus_count = mcfg_entry.bus_end as usize - mcfg_entry.bus_start as usize + 1;
        let region_size = bus_count * (1 << 20);

        let vmo = Vmo::create_physical(aligned_region_base_addr as usize, region_size)?;
        let vaddr = map_vmo(
            &vmo,
            0,
            region_size,
            MappingFlags::READ | MappingFlags::WRITE,
        )?;

        entries.push(AcpiMcfgEntry::new(
            *mcfg_entry,
            vaddr as usize + (region_base_addr as usize - aligned_region_base_addr as usize),
        ));
    }

    let pci_access = PciAccess::new(entries);
    for entry in mcfg_entries {
        let segment_group = entry.segment_group;

        (entry.bus_start..=entry.bus_end)
            .for_each(|bus| pci_scan_bus(segment_group, bus, &pci_access));
    }

    info!("PCI devices loaded");

    PCI_DEVICES
        .lock()
        .iter()
        .for_each(|device| debug!("{}", device));

    let pci_server = ServiceBuilder::new("pci")
        .build(PciDriverHandler)
        .map_err(|_| Error::new(EINVAL))?;

    pci_server.run().map_err(|_| Error::new(EINVAL))?;

    Ok(())
}
