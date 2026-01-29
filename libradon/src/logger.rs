use log::{Level, Record, set_logger, set_max_level};
use log::{LevelFilter, Log, Metadata};
use radon_kernel::nr::SYS_LOG;
use radon_kernel::syscall::log::{
    LOG_LEVEL_DEBUG, LOG_LEVEL_ERROR, LOG_LEVEL_INFO, LOG_LEVEL_WARN,
};

pub fn init() {
    set_logger(&Logger).unwrap();
    set_max_level(LevelFilter::Trace);
}

struct Logger;

impl Logger {
    fn log_message(&self, record: &Record) {
        if let Some(content) = record.args().as_str() {
            let log_level = match record.level() {
                Level::Error => LOG_LEVEL_ERROR,
                Level::Warn => LOG_LEVEL_WARN,
                Level::Info => LOG_LEVEL_INFO,
                Level::Debug => LOG_LEVEL_DEBUG,
                Level::Trace => LOG_LEVEL_INFO,
            };
            unsafe {
                crate::syscall::syscall3(
                    SYS_LOG,
                    log_level,
                    content.as_ptr() as usize,
                    content.len(),
                )
            };
        }
    }
}

impl Log for Logger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= Level::Trace
    }

    fn log(&self, record: &Record) {
        if self.enabled(record.metadata()) {
            self.log_message(record);
        }
    }

    fn flush(&self) {}
}
