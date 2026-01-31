use core::{hint::spin_loop, panic::PanicInfo, ptr::addr_of_mut};

use alloc::{boxed::Box, format};
use limine::request::ExecutableFileRequest;
use log::debug;
use object::{File, Object, ObjectSymbol};
use radon_kernel::object::process::current_process;
use rustc_demangle::demangle;
use spin::Lazy;
use unwinding::{
    abi::{_Unwind_Backtrace, _Unwind_GetIP, UnwindContext, UnwindReasonCode},
    panic::begin_panic,
};

#[used]
#[unsafe(link_section = ".requests")]
static EXE_REQUEST: ExecutableFileRequest = ExecutableFileRequest::new();

static KERNEL_FILE: Lazy<File> = Lazy::new(|| unsafe {
    let kernel = EXE_REQUEST.get_response().unwrap().file();
    let bin = core::slice::from_raw_parts(kernel.addr() as *const _, kernel.size() as _);
    File::parse(bin).expect("Failed to parse kernel file")
});

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    log::error!("{}", info);
    log::error!("Backtrace:");

    struct Counter(usize);

    extern "C" fn callback(
        unwind_ctx: &UnwindContext<'_>,
        arg: *mut core::ffi::c_void,
    ) -> UnwindReasonCode {
        let address = _Unwind_GetIP(unwind_ctx);
        let counter = unsafe { (arg as *mut Counter).as_mut() }.unwrap();

        let symbol = KERNEL_FILE
            .symbols()
            .find(|symbol| {
                let start = symbol.address();
                let end = start + symbol.size();
                (start..end).contains(&(address as u64))
            })
            .and_then(|symbol| symbol.name().ok())
            .map(|name| format!("{:#}", demangle(name)))
            .unwrap_or("<unknown>".into());

        log::error!("{:4}:{:#19x} -> {}", counter.0, address, symbol);
        counter.0 += 1;
        UnwindReasonCode::NO_REASON
    }

    let mut counter = Counter(0);
    _Unwind_Backtrace(callback, addr_of_mut!(counter) as *mut _);

    if info.can_unwind() {
        struct NoPayload;
        let code = begin_panic(Box::new(NoPayload));
        log::error!("Unwind reason code: {}", code.0);
    }

    if let Some(process) = current_process() {
        debug!("Faulting process name: {}", process.read().name());
    }

    loop {
        spin_loop();
    }
}
