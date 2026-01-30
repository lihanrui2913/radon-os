use core::fmt::{self, Write};

use alloc::format;
use log::{Level, Record, set_logger, set_max_level};
use log::{LevelFilter, Log, Metadata};
use radon_kernel::nr::SYS_LOG;
use spin::Mutex;

pub fn init() {
    set_logger(&Logger).unwrap();
    set_max_level(LevelFilter::Trace);
}

pub struct UserLoggerWriter;

impl Write for UserLoggerWriter {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        unsafe { crate::syscall::syscall2(SYS_LOG, s.as_ptr() as usize, s.len()) };
        Ok(())
    }
}

pub static LOCKED_KERNEL_WRITER: Mutex<UserLoggerWriter> = Mutex::new(UserLoggerWriter);

#[doc(hidden)]
pub fn _print(args: fmt::Arguments) {
    LOCKED_KERNEL_WRITER.lock().write_fmt(args).unwrap();
}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => (
        $crate::logger::_print(format_args!($($arg)*))
    )
}

#[macro_export]
macro_rules! println {
    () => ($crate::print!("\n"));
    ($($arg:tt)*) => (
        let __content = format!("{}\n", format_args!($($arg)*));
        $crate::print!("{}", __content)
    )
}

struct Logger;

impl Logger {
    fn log_message(&self, record: &Record, with_location: bool) {
        let color = match record.level() {
            Level::Error => "31",
            Level::Warn => "33",
            Level::Info => "32",
            Level::Debug => "34",
            Level::Trace => "35",
        };

        if with_location {
            let file = record.file().unwrap();
            let line = record.line().unwrap();
            crate::println!(
                "[{}] {}{}",
                format_args!("\x1b[{}m{}\x1b[0m", color, record.level().as_str()),
                record.args(),
                format_args!(", {}:{}", file, line)
            );
        } else {
            crate::println!(
                "[{}] {}",
                format_args!("\x1b[{}m{}\x1b[0m", color, record.level().as_str()),
                record.args(),
            );
        }
    }
}

impl Log for Logger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= Level::Trace
    }

    fn log(&self, record: &Record) {
        if self.enabled(record.metadata()) {
            let with_location = matches!(record.level(), Level::Debug);
            self.log_message(record, with_location);
        }
    }

    fn flush(&self) {}
}
