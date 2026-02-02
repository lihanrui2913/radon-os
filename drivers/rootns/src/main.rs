#![no_std]
#![no_main]
#![allow(unsafe_op_in_unsafe_fn)]

use alloc::{collections::btree_map::BTreeMap, string::ToString, vec};
use block_protocol::protocol::BLOCK_IOCTL_GETSIZE;
use deku::no_std_io::ErrorKind;
use efs::{dev::Device, fs::ext2::Ext2Fs};
use libdriver::{
    Request, RequestHandler, Response, RpcClient, ServiceBuilder,
    server::{ConnectionContext, RequestContext},
};
use libradon::{debug, error};
use namespace::{client::NamespaceClient, protocol::MountFlags};
use radon_kernel::{EINVAL, Error};

extern crate alloc;

/// Rootns 进程主入口
#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    match libradon::init() {
        Ok(()) => match rootns_main() {
            Ok(()) => {
                libradon::process::exit(0);
            }
            Err(_) => {
                error!("rootns: main function have some problems");
                libradon::process::exit(-1)
            }
        },
        Err(_) => libradon::process::exit(-1),
    }
}

struct Partition {
    inner: RpcClient,
}

impl Device for Partition {
    fn slice(
        &mut self,
        addr_range: core::ops::Range<efs::dev::address::Address>,
    ) -> deku::no_std_io::Result<efs::dev::Slice<'_>> {
        let mut buf = vec![0; addr_range.end.index() as usize - addr_range.start.index() as usize];
        self.inner
            .read(addr_range.start.index(), &mut buf)
            .map_err(|_| deku::no_std_io::Error::new(ErrorKind::InvalidInput, "I/O Error"))?;
        Ok(efs::dev::Slice::new_owned(buf, addr_range.start))
    }

    fn commit(&mut self, commit: efs::dev::Commit) -> deku::no_std_io::Result<()> {
        self.inner
            .write(commit.addr().index(), commit.as_ref())
            .map_err(|_| deku::no_std_io::Error::new(ErrorKind::InvalidInput, "I/O Error"))
            .map(|_| ())
    }

    fn size(&mut self) -> efs::dev::size::Size {
        efs::dev::size::Size(self.inner.ioctl(BLOCK_IOCTL_GETSIZE, 0).unwrap())
    }

    fn now(&mut self) -> Option<efs::fs::types::Timespec> {
        None
    }
}

pub struct RootNSRequestHandler(Ext2Fs<Partition>);

impl RequestHandler for RootNSRequestHandler {
    fn handle(&self, request: &Request, _ctx: &RequestContext) -> Response {
        Response::error(request.header.request_id, 1)
    }

    fn on_connect(&self, _ctx: &ConnectionContext) -> libdriver::Result<()> {
        Ok(())
    }

    fn on_disconnect(&self, _ctx: &ConnectionContext) {}
}

const MAX_PARTITION_NUM: usize = 32;
const ROOTNS_DRIVER_SERVICE_NAME: &'static str = "rootns";

fn rootns_main() -> radon_kernel::Result<()> {
    let mut finded = BTreeMap::new();

    'out: loop {
        if let Ok(partition_servers) =
            nameserver::client::list(Some("part"), MAX_PARTITION_NUM as u32)
        {
            for name in partition_servers.1.iter() {
                let driver_name = name.strip_prefix("driver.").unwrap();
                let key = driver_name.to_string();
                if finded.contains_key(&key) {
                    continue;
                }
                finded.insert(key, ());
                debug!("Finding root file system at {}", driver_name);
                if let Ok(rpc_client) = RpcClient::connect(driver_name) {
                    let partition = Partition { inner: rpc_client };
                    if let Ok(fs) = Ext2Fs::new(partition, 0) {
                        debug!("Found root file system at {}", driver_name);

                        let rootns_service = ServiceBuilder::new(ROOTNS_DRIVER_SERVICE_NAME)
                            .build(RootNSRequestHandler(fs))
                            .map_err(|_| Error::new(EINVAL))?;
                        NamespaceClient::connect()?.bind(
                            "/",
                            ROOTNS_DRIVER_SERVICE_NAME,
                            MountFlags::all(),
                        )?;

                        rootns_service.run().map_err(|_| Error::new(EINVAL))?;

                        break 'out;
                    }
                }
            }
        }
    }

    Ok(())
}
