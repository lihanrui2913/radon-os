use core::ptr::NonNull;

use acpi::AcpiTables;
use libradon::{
    memory::{MappingFlags, Vmo, map_vmo_at},
    syscall::clock_get,
};
use radon_kernel::{EINVAL, Error, Result};

pub const VA_BASE: usize = 0x4000_0000;

fn phys_to_virt(phys: usize) -> usize {
    phys + VA_BASE
}

#[derive(Clone)]
pub struct AcpiHandler;

#[allow(unused)]
impl ::acpi::Handler for AcpiHandler {
    unsafe fn map_physical_region<T>(
        &self,
        pa: usize,
        size: usize,
    ) -> acpi::PhysicalMapping<Self, T> {
        let aligned_pa = pa & !4095usize;
        let aligned_size = (size + 4095) & !4095usize;
        let offset = pa - aligned_pa;

        let va = phys_to_virt(pa);
        let aligned_va = va & !4095usize;

        let vmo = Vmo::create_physical(aligned_pa, aligned_size)
            .expect("No enougth memory to create VMO");
        let _ = map_vmo_at(
            &vmo,
            0,
            aligned_size,
            MappingFlags::READ | MappingFlags::WRITE,
            aligned_va as *mut u8,
        );

        acpi::PhysicalMapping {
            physical_start: pa,
            virtual_start: NonNull::new_unchecked(va as *mut T),
            region_length: size,
            mapped_length: size,
            handler: self.clone(),
        }
    }

    fn unmap_physical_region<T>(region: &acpi::PhysicalMapping<Self, T>) {}

    fn read_u8(&self, address: usize) -> u8 {
        unsafe { core::ptr::read_volatile(address as *const u8) }
    }

    fn read_u16(&self, address: usize) -> u16 {
        unsafe { core::ptr::read_volatile(address as *const u16) }
    }

    fn read_u32(&self, address: usize) -> u32 {
        unsafe { core::ptr::read_volatile(address as *const u32) }
    }

    fn read_u64(&self, address: usize) -> u64 {
        unsafe { core::ptr::read_volatile(address as *const u64) }
    }

    fn write_u8(&self, address: usize, value: u8) {
        unsafe {
            core::ptr::write_volatile(address as *mut _, value);
        }
    }
    fn write_u16(&self, address: usize, value: u16) {
        unsafe {
            core::ptr::write_volatile(address as *mut _, value);
        }
    }
    fn write_u32(&self, address: usize, value: u32) {
        unsafe {
            core::ptr::write_volatile(address as *mut _, value);
        }
    }
    fn write_u64(&self, address: usize, value: u64) {
        unsafe {
            core::ptr::write_volatile(address as *mut _, value);
        }
    }

    fn read_io_u8(&self, port: u16) -> u8 {
        0
    }

    fn read_io_u16(&self, port: u16) -> u16 {
        0
    }

    fn read_io_u32(&self, port: u16) -> u32 {
        0
    }

    fn write_io_u8(&self, port: u16, value: u8) {}

    fn write_io_u16(&self, port: u16, value: u16) {}

    fn write_io_u32(&self, port: u16, value: u32) {}

    fn read_pci_u8(&self, address: acpi::PciAddress, offset: u16) -> u8 {
        0
    }

    fn read_pci_u16(&self, address: acpi::PciAddress, offset: u16) -> u16 {
        0
    }

    fn read_pci_u32(&self, address: acpi::PciAddress, offset: u16) -> u32 {
        0
    }

    fn write_pci_u8(&self, address: acpi::PciAddress, offset: u16, value: u8) {}

    fn write_pci_u16(&self, address: acpi::PciAddress, offset: u16, value: u16) {}

    fn write_pci_u32(&self, address: acpi::PciAddress, offset: u16, value: u32) {}

    fn nanos_since_boot(&self) -> u64 {
        clock_get().unwrap() as u64
    }

    fn stall(&self, microseconds: u64) {}

    fn sleep(&self, milliseconds: u64) {}
}

pub struct Acpi {
    pub table: AcpiTables<AcpiHandler>,
}

impl Acpi {
    pub fn new(rsdp: usize) -> Result<Self> {
        Ok(Self {
            table: unsafe { AcpiTables::from_rsdp(AcpiHandler, rsdp) }
                .map_err(|_| Error::new(EINVAL))?,
        })
    }
}
