use alloc::string::String;
use libdriver::{DriverClient, DriverOp};
use libradon::{channel::Channel, handle::OwnedHandle, memory::Vmo};
use namespace::protocol::{
    NAMESPACE_INVALID_ARGUMENT, NAMESPACE_RESOLVE_FAILED, NAMESPACE_UNKNOWN_OP,
};
use radon_kernel::{EINVAL, EIO, ENOENT, Error, Result};

pub fn namespace_error_to_error(err: i32) -> Result<()> {
    match err {
        0 => Ok(()),
        NAMESPACE_INVALID_ARGUMENT => Err(Error::new(EINVAL)),
        NAMESPACE_UNKNOWN_OP => Err(Error::new(EINVAL)),
        NAMESPACE_RESOLVE_FAILED => Err(Error::new(ENOENT)),
        _ => Err(Error::new(EINVAL)),
    }
}

pub fn open_inner(path: String) -> Result<(Vmo, i32)> {
    let client = DriverClient::connect("namespace").map_err(|_| Error::new(EIO))?;
    let open_response = client
        .call(DriverOp::Open, path.as_bytes())
        .map_err(|_| Error::new(EIO))?;
    namespace_error_to_error(open_response.header.status)?;
    let fs_handle = open_response.handles.get(0).ok_or(Error::new(ENOENT))?;
    let fs_channel = Channel::from_handle(OwnedHandle::from_raw(fs_handle.raw()));
    let driver_client = DriverClient::from_channel(fs_channel).map_err(|_| Error::new(EINVAL))?;
    let response = driver_client
        .call(DriverOp::Open, &open_response.data)
        .map_err(|_| Error::new(EIO))?;
    namespace_error_to_error(response.header.status)?;
    let handle = response.handles.get(0).ok_or(Error::new(ENOENT))?;
    let result = (
        Vmo::from_handle(OwnedHandle::from_raw(handle.raw())),
        unsafe { (response.data.as_ptr() as *const i32).read_unaligned() },
    );
    Ok(result)
}
