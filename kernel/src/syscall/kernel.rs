use crate::{ENOENT, Error, Result, drivers::acpi::RSDP_REQUEST};

pub fn get_rsdp() -> Result<usize> {
    RSDP_REQUEST
        .get_response()
        .ok_or(Error::new(ENOENT))
        .map(|rsdp_response| rsdp_response.address())
}
