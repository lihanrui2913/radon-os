use alloc::{string::String, vec::Vec};
use libdriver::{DriverClient, DriverOp};
use libradon::{channel::Channel, handle::OwnedHandle};
use radon_kernel::{EEXIST, EINVAL, ENOENT, Error, Result};

use crate::protocol::MountFlags;

pub struct NamespaceClient {
    client: DriverClient,
}

impl NamespaceClient {
    pub fn connect() -> Result<Self> {
        let client = libdriver::client::DriverClient::connect("namespace")
            .map_err(|_| Error::new(ENOENT))?;

        Ok(Self { client })
    }

    pub fn bind(&self, path: &str, name: &str, flags: MountFlags) -> Result<()> {
        let mut buf = Vec::new();
        buf.extend_from_slice(&flags.bits().to_le_bytes());
        buf.extend_from_slice(&path.len().to_le_bytes());
        buf.extend_from_slice(path.as_bytes());
        buf.extend_from_slice(name.as_bytes());
        self.client
            .call(DriverOp::UserDefined, &buf)
            .map_err(|_| Error::new(EEXIST))?;
        Ok(())
    }

    pub fn open(&self, path: &str) -> Result<(Channel, String)> {
        let response = self
            .client
            .call(DriverOp::Open, path.as_bytes())
            .map_err(|_| Error::new(ENOENT))?;
        let remaining_path = String::from_utf8(response.data).map_err(|_| Error::new(EINVAL))?;
        let handle =
            OwnedHandle::from_raw(response.handles.get(0).ok_or(Error::new(EINVAL))?.raw());
        let channel = Channel::from_handle(handle);
        Ok((channel, remaining_path))
    }
}
