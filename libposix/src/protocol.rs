pub const POSIX_CALL_READ: usize = 1;
pub const POSIX_CALL_WRITE: usize = 2;
pub const POSIX_CALL_OPEN: usize = 3;
pub const POSIX_CALL_CLOSE: usize = 4;
pub const POSIX_CALL_NEWFSTATAT: usize = 5;
pub const POSIX_CALL_LSEEK: usize = 6;

pub const POSIX_CALL_GETPID: usize = 40;
pub const POSIX_CALL_GETPPID: usize = 41;
pub const POSIX_CALL_SETPPID: usize = 42;
pub const POSIX_CALL_GETTID: usize = 43;
pub const POSIX_CALL_GETSID: usize = 44;
pub const POSIX_CALL_SETSID: usize = 45;
pub const POSIX_CALL_GETPGID: usize = 46;
pub const POSIX_CALL_GETRESUID: usize = 47;
pub const POSIX_CALL_GETRESGID: usize = 48;
pub const POSIX_CALL_SETRESUID: usize = 49;
pub const POSIX_CALL_SETRESGID: usize = 50;

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct PosixRequest {
    pub idx: usize,
    pub arg1: usize,
    pub arg2: usize,
    pub arg3: usize,
    pub arg4: usize,
    pub arg5: usize,
    pub arg6: usize,
}

impl PosixRequest {
    pub fn to_bytes(&self) -> &[u8] {
        unsafe { core::slice::from_raw_parts(self as *const _ as *const u8, size_of::<Self>()) }
    }

    pub fn to_bytes_mut(&mut self) -> &mut [u8] {
        unsafe { core::slice::from_raw_parts_mut(self as *mut _ as *mut u8, size_of::<Self>()) }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct PosixResponse {
    pub ret: usize,
}

impl PosixResponse {
    pub fn to_bytes(&self) -> &[u8] {
        unsafe { core::slice::from_raw_parts(self as *const _ as *const u8, size_of::<Self>()) }
    }

    pub fn to_bytes_mut(&mut self) -> &mut [u8] {
        unsafe { core::slice::from_raw_parts_mut(self as *mut _ as *mut u8, size_of::<Self>()) }
    }
}
