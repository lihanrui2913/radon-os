pub mod channel;
pub mod handle;
pub mod port;
pub mod process;
pub mod signal;
pub mod vmar;
pub mod vmo;
pub mod wait_queue;

pub use channel::{Channel, Message};
pub use handle::{Handle, HandleEntry, HandleTable, Rights};
pub use port::{BindOptions, PacketType, Port, PortPacket};
pub use process::{ArcProcess, Process, WeakArcProcess, layout};
pub use signal::Signals;
pub use wait_queue::WaitQueue;

use alloc::sync::Arc;
use alloc::vec::Vec;
use core::any::Any;

/// 对象类型
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectType {
    None = 0,
    Port = 1,
    Channel = 2,
    Event = 3,
    Timer = 4,
    Vmo = 5,
    Process = 6,
    Thread = 7,
    Vmar = 9,
}

/// 信号观察者
pub struct SignalObserver {
    pub key: u64,
    pub trigger_signals: Signals,
    pub callback: Arc<dyn Fn(Signals) + Send + Sync>,
    pub once: bool,
}

/// 所有内核对象的 trait
pub trait KernelObject: Any + Send + Sync + 'static {
    fn object_type(&self) -> ObjectType;
    fn signals(&self) -> Signals;
    fn signal_set(&self, signals: Signals);
    fn signal_clear(&self, signals: Signals);
    fn add_signal_observer(&self, observer: SignalObserver);
    fn remove_signal_observer(&self, key: u64);
    fn as_any(&self) -> &dyn Any;
}

/// 信号状态管理
pub struct SignalState {
    signals: Signals,
    observers: Vec<SignalObserver>,
}

impl SignalState {
    pub fn new() -> Self {
        Self {
            signals: Signals::empty(),
            observers: Vec::new(),
        }
    }

    pub fn get(&self) -> Signals {
        self.signals
    }

    pub fn set(&mut self, signals: Signals) {
        let old = self.signals;
        self.signals |= signals;
        let changed = self.signals & !old;

        if !changed.is_empty() {
            self.notify(changed);
        }
    }

    pub fn clear(&mut self, signals: Signals) {
        self.signals &= !signals;
    }

    pub fn add_observer(&mut self, observer: SignalObserver) {
        let current = self.signals;
        if current.intersects(observer.trigger_signals) {
            let triggered = current & observer.trigger_signals;
            (observer.callback)(triggered);
            if observer.once {
                return;
            }
        }
        self.observers.push(observer);
    }

    pub fn remove_observer(&mut self, key: u64) {
        self.observers.retain(|o| o.key != key);
    }

    fn notify(&mut self, changed: Signals) {
        let mut to_remove = Vec::new();

        for (i, observer) in self.observers.iter().enumerate() {
            if changed.intersects(observer.trigger_signals) {
                (observer.callback)(changed & observer.trigger_signals);
                if observer.once {
                    to_remove.push(i);
                }
            }
        }

        for i in to_remove.into_iter().rev() {
            self.observers.remove(i);
        }
    }
}

#[macro_export]
macro_rules! get_object_as {
    ($handle:expr, $rights:expr, $type:ty) => {{
        use $crate::object::process::current_process;
        use $crate::object::{Handle, Rights};
        use $crate::syscall::error::{EBADF, EINVAL, Error};

        let process = current_process().ok_or(Error::new(EINVAL))?;
        let proc = process.read();
        let obj = proc
            .handles()
            .get(Handle::from($handle), $rights)
            .ok_or(Error::new(EBADF))?;

        // 尝试 downcast
        let result: Option<&$type> = obj.as_any().downcast_ref::<$type>();
        if result.is_none() {
            return Err(Error::new(EINVAL));
        }

        obj
    }};
}
