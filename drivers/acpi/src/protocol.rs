pub const ACPI_DAEMON_STATUS_OK: i32 = 0;
pub const ACPI_DAEMON_STATUS_NOT_FOUND: i32 = 1;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct AcpiMcfg {
    pub base_address: u64,
    pub segment_group: u16,
    pub bus_start: u8,
    pub bus_end: u8,
}

impl AcpiMcfg {
    pub fn to_bytes(&self) -> &[u8] {
        unsafe { core::slice::from_raw_parts(self as *const _ as *const u8, size_of::<Self>()) }
    }
}
