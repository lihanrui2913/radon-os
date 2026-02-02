#![no_std]
#![no_main]
#![allow(unsafe_op_in_unsafe_fn)]

use alloc::{
    string::{String, ToString},
    sync::Arc,
    vec,
};
use libdriver::{
    DriverOp, Request, RequestHandler, Response, ServiceBuilder,
    server::{ConnectionContext, RequestContext},
};
use libradon::error;
use namespace::protocol::{
    NAMESPACE_BIND_FAILED, NAMESPACE_INTERNAL_ERROR, NAMESPACE_INVALID_ARGUMENT,
    NAMESPACE_RESOLVE_FAILED, NAMESPACE_UNKNOWN_OP,
};
use radon_kernel::{EINVAL, Error};

use namespace::protocol::MountFlags;
use server::Namespace;

extern crate alloc;

mod server;

/// Namespace 进程主入口
#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    match libradon::init() {
        Ok(()) => match namespace_main() {
            Ok(()) => {
                libradon::process::exit(0);
            }
            Err(_) => {
                error!("namespace: main function have some problems");
                libradon::process::exit(-1)
            }
        },
        Err(_) => libradon::process::exit(-1),
    }
}

struct NamespaceServiceHandler(Arc<Namespace>);

impl RequestHandler for NamespaceServiceHandler {
    fn handle(&self, request: &Request, _ctx: &RequestContext) -> Response {
        let op = DriverOp::from(request.header.op);
        match op {
            DriverOp::Open => {
                if let Ok(path) = String::from_utf8(request.data.clone()) {
                    if let Ok((entry, remaining)) = self.0.resolve(path) {
                        if let Ok(mut client) =
                            libdriver::client::DriverClient::connect(&entry.name)
                        {
                            client.with_nodrop(true);
                            let handles = vec![client.channel().handle()];
                            Response::success(request.header.request_id)
                                .with_data(remaining.as_bytes().to_vec())
                                .with_handles(handles)
                        } else {
                            Response::error(request.header.request_id, NAMESPACE_INTERNAL_ERROR)
                        }
                    } else {
                        Response::error(request.header.request_id, NAMESPACE_RESOLVE_FAILED)
                    }
                } else {
                    Response::error(request.header.request_id, NAMESPACE_INVALID_ARGUMENT)
                }
            }
            DriverOp::UserDefined => {
                if request.data.len() < 4 {
                    Response::error(request.header.request_id, NAMESPACE_INVALID_ARGUMENT)
                } else {
                    let flags =
                        unsafe { (request.data[..4].as_ptr() as *const u32).read_unaligned() };
                    let path_len =
                        unsafe { (request.data[4..8].as_ptr() as *const u32).read_unaligned() }
                            as usize;
                    if let Ok(path) = str::from_utf8(&request.data[8..(8 + path_len)])
                        && let Ok(name) = str::from_utf8(&request.data[(8 + path_len)..])
                    {
                        if let Ok(_) = self.0.bind(
                            path,
                            name.to_string(),
                            MountFlags::from_bits_truncate(flags),
                        ) {
                            Response::success(request.header.request_id)
                        } else {
                            Response::error(request.header.request_id, NAMESPACE_BIND_FAILED)
                        }
                    } else {
                        Response::error(request.header.request_id, NAMESPACE_INVALID_ARGUMENT)
                    }
                }
            }
            _ => Response::error(request.header.request_id, NAMESPACE_UNKNOWN_OP),
        }
    }

    fn on_connect(&self, _ctx: &ConnectionContext) -> libdriver::Result<()> {
        Ok(())
    }

    fn on_disconnect(&self, _ctx: &ConnectionContext) {}
}

fn namespace_main() -> radon_kernel::Result<()> {
    let namespace_server = ServiceBuilder::new("namespace")
        .build(NamespaceServiceHandler(Arc::new(Namespace::new())))
        .map_err(|_| Error::new(EINVAL))
        .expect("Failed to build service");

    namespace_server.run().map_err(|_| Error::new(EINVAL))?;

    Ok(())
}
