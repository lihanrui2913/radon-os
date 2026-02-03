use core::mem::offset_of;

use bitflags::bitflags;

pub const NAMESPACE_UNKNOWN_OP: i32 = 1;
pub const NAMESPACE_INVALID_ARGUMENT: i32 = 2;
pub const NAMESPACE_BIND_FAILED: i32 = 3;
pub const NAMESPACE_RESOLVE_FAILED: i32 = 4;
pub const NAMESPACE_INTERNAL_ERROR: i32 = 5;

pub const NAMESPACE_FILE_TYPE_UNKNOWN: i32 = 0;
pub const NAMESPACE_FILE_TYPE_REGULAR: i32 = 1;
pub const NAMESPACE_FILE_TYPE_DIRECTORY: i32 = 2;
pub const NAMESPACE_FILE_TYPE_SYMLINK: i32 = 3;

bitflags! {
    #[derive(Debug, Clone, Copy)]
    pub struct MountFlags: u32 {
        const READABLE   = 0b0001;
        const WRITABLE   = 0b0010;
        const EXECUTABLE = 0b0100;
        const ADMIN      = 0b1000;
    }
}

#[repr(C)]
pub struct NsDirEntry {
    pub rec_len: usize,
    pub name_len: usize,
    pub file_type: i32,
    pub name: [u8; 256],
}

impl NsDirEntry {
    pub fn to_bytes(&self) -> &[u8] {
        unsafe {
            core::slice::from_raw_parts(self as *const _ as *const u8, offset_of!(NsDirEntry, name))
        }
    }
}
