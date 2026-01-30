use pci_types::MAX_BARS;

pub const PCI_STATUS_OK: i32 = 0;
pub const PCI_STATUS_NOT_FOUND: i32 = 1;

pub const BAR_TYPE_IO: u8 = 1;
pub const BAR_TYPE_MMIO: u8 = 2;

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct BarInfo {
    pub address: u64,
    pub size: u32,
    pub bar_type: u8,
}

impl BarInfo {
    pub fn to_bytes(&self) -> &[u8] {
        unsafe { core::slice::from_raw_parts(self as *const _ as *const u8, size_of::<Self>()) }
    }
}

impl core::fmt::Display for BarInfo {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        write!(f, "{:#016x}@{:#08x}", self.address, self.size,)
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct PciDeviceInfo {
    pub bars: [BarInfo; MAX_BARS],
    pub class: u8,
    pub subclass: u8,
    pub interface: u8,
    pub vendor: u16,
    pub device: u16,
    pub subsystem_vendor: u16,
    pub subsystem_device: u16,
    pub revision: u8,
}

impl PciDeviceInfo {
    pub fn from_bytes(bytes: &[u8]) -> Self {
        *unsafe { (bytes.as_ptr() as *const Self).as_ref() }.unwrap()
    }

    pub fn to_bytes(&self) -> &[u8] {
        unsafe { core::slice::from_raw_parts(self as *const _ as *const u8, size_of::<Self>()) }
    }
}

impl core::fmt::Display for PciDeviceInfo {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        write!(
            f,
            "[{:04x}:{:04x}] [{:04x}:{:04x}] (rev: {:02x})",
            self.vendor, self.device, self.subsystem_vendor, self.subsystem_device, self.revision,
        )
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct PciGetDeviceInfoRequest {
    pub class: u8,
    pub subclass: u8,
    pub interface: u8,
    pub vendor: u16,
    pub device: u16,
    pub subsystem_vendor: u16,
    pub subsystem_device: u16,
}

impl PciGetDeviceInfoRequest {
    pub fn from_bytes(bytes: &[u8]) -> Self {
        *unsafe { (bytes.as_ptr() as *const Self).as_ref() }.unwrap()
    }

    pub fn to_bytes(&self) -> &[u8] {
        unsafe { core::slice::from_raw_parts(self as *const _ as *const u8, size_of::<Self>()) }
    }
}
