#![no_std]

extern crate alloc;

pub mod protocol;

#[cfg(feature = "client")]
pub mod client;

#[cfg(feature = "handler")]
pub mod handler;

pub use protocol::*;

#[cfg(feature = "client")]
pub use client::{BootstrapClient, BootstrapError, get_nameserver, get_service};

#[cfg(feature = "handler")]
pub use handler::BootstrapHandler;
