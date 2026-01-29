use crate::{
    arch::{Ptrace, irq::IrqRegsArch},
    syscall::error::{ENOSYS, Error},
};

pub mod clock;
pub mod error;
pub mod kernel;
pub mod log;
pub mod memory;
pub mod nr;
pub mod object;
pub mod process;

use nr::*;

pub extern "C" fn syscall_handler(regs: *mut Ptrace) {
    let regs = unsafe { regs.as_mut_unchecked() };

    let idx = regs.get_syscall_idx() as usize;
    let (arg1, arg2, arg3, arg4, arg5, arg6) = regs.get_syscall_args();
    let (arg1, arg2, arg3, arg4, arg5, arg6) = (
        arg1 as usize,
        arg2 as usize,
        arg3 as usize,
        arg4 as usize,
        arg5 as usize,
        arg6 as usize,
    );

    // crate::serial_println!(
    //     "syscall({:#x}): ({:#x}, {:#x}, {:#x}, {:#x}, {:#x}, {:#x})",
    //     idx,
    //     arg1,
    //     arg2,
    //     arg3,
    //     arg4,
    //     arg5,
    //     arg6
    // );

    let ret = match idx {
        SYS_LOG => log::sys_log(arg1, arg2, arg3),

        SYS_HANDLE_CLOSE => object::sys_handle_close(arg1),
        SYS_HANDLE_DUPLICATE => object::sys_handle_duplicate(arg1, arg2),

        SYS_PORT_CREATE => object::sys_port_create(),
        SYS_PORT_WAIT => object::sys_port_wait(arg1, arg2, arg3, arg4),
        SYS_PORT_BIND => object::sys_port_bind(arg1, arg2, arg3, arg4, arg5),
        SYS_PORT_UNBIND => object::sys_port_unbind(arg1, arg2),
        SYS_PORT_QUEUE => object::sys_port_queue(arg1, arg2, arg3),

        SYS_CHANNEL_CREATE => object::sys_channel_create(arg1),
        SYS_CHANNEL_SEND => object::sys_channel_send(arg1, arg2, arg3, arg4, arg5),
        SYS_CHANNEL_RECV => object::sys_channel_recv(arg1, arg2, arg3, arg4, arg5, arg6),
        SYS_CHANNEL_TRY_RECV => object::sys_channel_try_recv(arg1, arg2, arg3, arg4, arg5, arg6),

        SYS_CLOCK_GET => clock::sys_clock_get(),

        SYS_PROCESS_CREATE => process::sys_process_create(arg1, arg2),
        SYS_PROCESS_START => process::sys_process_start(arg1),
        SYS_THREAD_CREATE => process::sys_thread_create(arg1, arg2),
        SYS_EXIT => process::sys_exit(arg1),
        SYS_PROCESS_GET_INIT_HANDLE => process::sys_process_get_init_handle(arg1),
        SYS_PROCESS_WAIT => process::sys_process_wait(arg1, arg2, arg3),
        SYS_PROCESS_GET_VMAR_HANDLE => process::sys_process_get_vmar_handle(arg1),

        SYS_VMO_CREATE => memory::sys_vmo_create(arg1, arg2),
        SYS_VMO_CREATE_PHYSICAL => memory::sys_vmo_create_physical(arg1, arg2, arg3),
        SYS_VMO_CREATE_CHILD => memory::sys_vmo_create_child(arg1, arg2, arg3, arg4),
        SYS_VMO_READ => memory::sys_vmo_read(arg1, arg2, arg3, arg4),
        SYS_VMO_WRITE => memory::sys_vmo_write(arg1, arg2, arg3, arg4),
        SYS_VMO_GET_SIZE => memory::sys_vmo_get_size(arg1),
        SYS_VMO_SET_SIZE => memory::sys_vmo_set_size(arg1, arg2),
        SYS_VMO_GET_PHYS => memory::sys_vmo_get_phys(arg1),

        SYS_VMAR_MAP => memory::sys_vmar_map(arg1, arg2),
        SYS_VMAR_UNMAP => memory::sys_vmar_unmap(arg1, arg2, arg3),
        SYS_VMAR_PROTECT => memory::sys_vmar_protect(arg1, arg2, arg3, arg4),

        SYS_YIELD => {
            crate::task::schedule();
            Ok(0)
        }

        SYS_KRES_GET_RSDP => kernel::get_rsdp(),

        _ => {
            warn!("Syscall {} not implemented", idx);
            Err(Error::new(ENOSYS))
        }
    };

    regs.set_ret_value(Error::mux(ret) as u64);
}
