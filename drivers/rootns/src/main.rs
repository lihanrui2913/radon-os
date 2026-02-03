#![no_std]
#![no_main]
#![allow(unsafe_op_in_unsafe_fn)]

use core::{mem::offset_of, str::FromStr};

use alloc::{
    collections::btree_map::BTreeMap,
    string::{String, ToString},
    vec,
    vec::Vec,
};
use block_protocol::protocol::BLOCK_IOCTL_GETSIZE;
use deku::no_std_io::{ErrorKind, Read, Seek};
use efs::{
    dev::Device,
    fs::{
        FilesystemRead,
        ext2::{Ext2Fs, Ext2TypeWithFile},
        file::DirectoryRead,
    },
    path::Path,
};
use libdriver::{
    DriverOp, Request, RequestHandler, Response, RpcClient, ServiceBuilder,
    server::{ConnectionContext, RequestContext},
};
use libradon::{
    debug, error,
    memory::{Vmo, VmoOptions},
};
use namespace::{
    client::NamespaceClient,
    protocol::{
        MountFlags, NAMESPACE_FILE_TYPE_DIRECTORY, NAMESPACE_FILE_TYPE_REGULAR,
        NAMESPACE_FILE_TYPE_SYMLINK, NAMESPACE_FILE_TYPE_UNKNOWN, NAMESPACE_INTERNAL_ERROR,
        NAMESPACE_INVALID_ARGUMENT, NAMESPACE_RESOLVE_FAILED, NsDirEntry,
    },
};
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

pub struct RootNSRequestHandler {
    inner: Ext2Fs<Partition>,
}

impl RequestHandler for RootNSRequestHandler {
    fn handle(&self, request: &Request, _ctx: &RequestContext) -> Response {
        let op = DriverOp::from(request.header.op);
        match op {
            DriverOp::Open => {
                let string = match String::from_utf8(request.data.clone()) {
                    Ok(s) => s,
                    Err(_) => {
                        return Response::error(
                            request.header.request_id,
                            NAMESPACE_INVALID_ARGUMENT,
                        );
                    }
                };
                let path = match Path::from_str(&string) {
                    Ok(p) => p,
                    Err(_) => {
                        return Response::error(
                            request.header.request_id,
                            NAMESPACE_INVALID_ARGUMENT,
                        );
                    }
                };
                let file = match self.inner.get_file(
                    &path,
                    self.inner.root().expect("File system is broken"),
                    true,
                ) {
                    Ok(f) => f,
                    Err(_) => {
                        return Response::error(
                            request.header.request_id,
                            NAMESPACE_RESOLVE_FAILED,
                        );
                    }
                };
                let (handle, file_ty) = match file {
                    Ext2TypeWithFile::Regular(mut regular) => {
                        if regular.size().0 == 0 {
                            let mut vmo = match Vmo::create(
                                4096usize,
                                VmoOptions::COMMIT | VmoOptions::RESIZABLE,
                            ) {
                                Ok(v) => v,
                                Err(_) => {
                                    return Response::error(
                                        request.header.request_id,
                                        NAMESPACE_INTERNAL_ERROR,
                                    );
                                }
                            };
                            vmo.with_nodrop(true);
                            (vmo.handle(), NAMESPACE_FILE_TYPE_REGULAR)
                        } else {
                            let mut vmo = match Vmo::create(
                                (regular.size().0 as usize + 4095usize) & !4095usize,
                                VmoOptions::COMMIT | VmoOptions::RESIZABLE,
                            ) {
                                Ok(v) => v,
                                Err(_) => {
                                    return Response::error(
                                        request.header.request_id,
                                        NAMESPACE_INTERNAL_ERROR,
                                    );
                                }
                            };
                            let mut offset = 0;
                            let mut tmp = vec![0u8; 4096];
                            while offset < regular.size().0 as usize {
                                if let Err(_) =
                                    regular.seek(deku::no_std_io::SeekFrom::Start(offset as u64))
                                {
                                    return Response::error(
                                        request.header.request_id,
                                        NAMESPACE_INTERNAL_ERROR,
                                    );
                                }
                                if let Err(_) = regular.read(&mut tmp) {
                                    return Response::error(
                                        request.header.request_id,
                                        NAMESPACE_INTERNAL_ERROR,
                                    );
                                }
                                if let Err(_) = vmo.write(offset, &tmp) {
                                    return Response::error(
                                        request.header.request_id,
                                        NAMESPACE_INTERNAL_ERROR,
                                    );
                                }
                                offset += tmp.len();
                            }
                            vmo.with_nodrop(true);
                            (vmo.handle(), NAMESPACE_FILE_TYPE_REGULAR)
                        }
                    }
                    Ext2TypeWithFile::Directory(directory) => {
                        let mut dentries = Vec::new();

                        let entries = match directory.entries() {
                            Ok(e) => e,
                            Err(_) => {
                                return Response::error(
                                    request.header.request_id,
                                    NAMESPACE_INTERNAL_ERROR,
                                );
                            }
                        };

                        for entry in entries.iter() {
                            let dentry_len = offset_of!(NsDirEntry, name) + entry.filename.len();
                            let mut dentry = Vec::with_capacity(dentry_len);
                            let file_type = if entry.file.is_directory() {
                                NAMESPACE_FILE_TYPE_DIRECTORY
                            } else if entry.file.is_regular() {
                                NAMESPACE_FILE_TYPE_REGULAR
                            } else if entry.file.is_symlink() {
                                NAMESPACE_FILE_TYPE_SYMLINK
                            } else {
                                NAMESPACE_FILE_TYPE_UNKNOWN
                            };
                            dentry.extend_from_slice(
                                NsDirEntry {
                                    rec_len: dentry_len,
                                    name_len: entry.filename.len(),
                                    file_type,
                                    name: [0u8; 256],
                                }
                                .to_bytes(),
                            );
                            dentry.extend_from_slice(entry.filename.as_bytes());

                            dentries.extend_from_slice(&dentry);
                        }

                        let mut vmo = match Vmo::create(
                            (dentries.len() + 4095usize) & !4095usize,
                            VmoOptions::COMMIT | VmoOptions::RESIZABLE,
                        ) {
                            Ok(v) => v,
                            Err(_) => {
                                return Response::error(
                                    request.header.request_id,
                                    NAMESPACE_INTERNAL_ERROR,
                                );
                            }
                        };
                        if let Err(_) = vmo.write(0, &dentries) {
                            return Response::error(
                                request.header.request_id,
                                NAMESPACE_INTERNAL_ERROR,
                            );
                        }
                        vmo.with_nodrop(true);
                        (vmo.handle(), NAMESPACE_FILE_TYPE_DIRECTORY)
                    }
                    _ => {
                        return Response::error(
                            request.header.request_id,
                            NAMESPACE_INTERNAL_ERROR,
                        );
                    }
                };

                Response::success(request.header.request_id)
                    .with_data(file_ty.to_le_bytes().to_vec())
                    .with_handles(vec![handle])
            }
            _ => Response::error(request.header.request_id, 1),
        }
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

                        NamespaceClient::connect()?.bind(
                            "/",
                            ROOTNS_DRIVER_SERVICE_NAME,
                            MountFlags::all(),
                        )?;

                        let rootns_service = ServiceBuilder::new(ROOTNS_DRIVER_SERVICE_NAME)
                            .build(RootNSRequestHandler { inner: fs })
                            .map_err(|_| Error::new(EINVAL))?;
                        rootns_service.run().map_err(|_| Error::new(EINVAL))?;

                        break 'out;
                    }
                }
            }
        }

        libradon::syscall::nanosleep(1000_000_000)?;
    }

    Ok(())
}
