use bitflags::bitflags;

pub const NAMESPACE_UNKNOWN_OP: i32 = 1;
pub const NAMESPACE_INVALID_ARGUMENT: i32 = 2;
pub const NAMESPACE_BIND_FAILED: i32 = 3;
pub const NAMESPACE_RESOLVE_FAILED: i32 = 4;
pub const NAMESPACE_INTERNAL_ERROR: i32 = 5;

bitflags! {
    #[derive(Debug, Clone, Copy)]
    pub struct MountFlags: u32 {
        const READABLE   = 0b0001;
        const WRITABLE   = 0b0010;
        const EXECUTABLE = 0b0100;
        const ADMIN      = 0b1000;
    }
}
