use alloc::collections::VecDeque;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use core::any::Any;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

use crate::arch::CurrentTimeArch;
use crate::arch::time::TimeArch;

use super::{KernelObject, ObjectType, SignalObserver, Signals, wait_queue::WaitQueue};

/// 事件包
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct PortPacket {
    /// 用户 key
    pub key: u64,
    /// 触发的信号
    pub signals: Signals,
    /// 包类型
    pub packet_type: PacketType,
    /// 保留字段
    pub reserved: u32,
    /// 用户数据
    pub data: [u64; 4],
}

#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacketType {
    Signal = 0,
    User = 1,
    Timer = 2,
}

impl PortPacket {
    pub const fn zeroed() -> Self {
        Self {
            key: 0,
            signals: Signals::empty(),
            packet_type: PacketType::User,
            reserved: 0,
            data: [0; 4],
        }
    }

    pub fn signal(key: u64, signals: Signals) -> Self {
        Self {
            key,
            signals,
            packet_type: PacketType::Signal,
            reserved: 0,
            data: [0; 4],
        }
    }

    pub fn user(key: u64, data: [u64; 4]) -> Self {
        Self {
            key,
            signals: Signals::empty(),
            packet_type: PacketType::User,
            reserved: 0,
            data,
        }
    }
}

/// 绑定选项
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindOptions {
    Once = 0,
    Persistent = 1,
}

impl From<u32> for BindOptions {
    fn from(v: u32) -> Self {
        match v {
            0 => BindOptions::Once,
            _ => BindOptions::Persistent,
        }
    }
}

/// 绑定记录
struct Binding {
    key: u64,
    object: Arc<dyn KernelObject>,
    trigger_signals: Signals,
    options: BindOptions,
}

/// Port 内部状态
struct PortInner {
    /// 事件队列
    packets: VecDeque<PortPacket>,
    /// 绑定列表
    bindings: Vec<Binding>,
    /// 当前信号
    signals: Signals,
    /// 信号观察者
    observers: Vec<SignalObserver>,
}

/// Port 对象
pub struct Port {
    inner: Mutex<PortInner>,
    waiters: WaitQueue,
    next_key: AtomicU64,
    self_weak: Mutex<Option<Weak<Port>>>,
}

impl Port {
    /// 创建新的 Port
    pub fn new() -> Arc<Self> {
        let port = Arc::new(Self {
            inner: Mutex::new(PortInner {
                packets: VecDeque::new(),
                bindings: Vec::new(),
                signals: Signals::empty(),
                observers: Vec::new(),
            }),
            waiters: WaitQueue::new(),
            next_key: AtomicU64::new(1),
            self_weak: Mutex::new(None),
        });

        *port.self_weak.lock() = Some(Arc::downgrade(&port));
        port
    }

    /// 分配 key
    pub fn alloc_key(&self) -> u64 {
        self.next_key.fetch_add(1, Ordering::Relaxed)
    }

    /// 绑定对象
    pub fn bind(
        self: &Arc<Self>,
        key: u64,
        object: Arc<dyn KernelObject>,
        trigger_signals: Signals,
        options: BindOptions,
    ) -> Result<(), PortError> {
        // 检查 key 冲突
        {
            let inner = self.inner.lock();
            if inner.bindings.iter().any(|b| b.key == key) {
                return Err(PortError::AlreadyBound);
            }
        }

        // 设置观察者回调
        let port_weak = self.self_weak.lock().clone().unwrap();
        let callback_key = key;
        let once = options == BindOptions::Once;

        let observer = SignalObserver {
            key,
            trigger_signals,
            callback: Arc::new(move |signals| {
                if let Some(port) = port_weak.upgrade() {
                    port.on_object_signal(callback_key, signals);
                }
            }),
            once,
        };

        object.add_signal_observer(observer);

        // 保存绑定
        let mut inner = self.inner.lock();
        inner.bindings.push(Binding {
            key,
            object,
            trigger_signals,
            options,
        });

        Ok(())
    }

    /// 解绑
    pub fn unbind(&self, key: u64) -> Result<(), PortError> {
        let mut inner = self.inner.lock();

        let pos = inner
            .bindings
            .iter()
            .position(|b| b.key == key)
            .ok_or(PortError::NotFound)?;

        let binding = inner.bindings.remove(pos);
        binding.object.remove_signal_observer(key);

        Ok(())
    }

    /// 对象信号触发时的回调
    fn on_object_signal(&self, key: u64, signals: Signals) {
        let mut inner = self.inner.lock();

        // 创建事件包
        let packet = PortPacket::signal(key, signals);
        inner.packets.push_back(packet);

        // 更新 Port 信号
        inner.signals |= Signals::READABLE;

        // 通知观察者
        let observers_to_notify: Vec<_> = inner
            .observers
            .iter()
            .filter(|o| o.trigger_signals.contains(Signals::READABLE))
            .map(|o| o.callback.clone())
            .collect();

        drop(inner);

        // 回调
        for callback in observers_to_notify {
            callback(Signals::READABLE);
        }

        // 唤醒等待者
        self.waiters.wake_one();
    }

    /// 手动投递事件
    pub fn queue(&self, packet: PortPacket) {
        {
            let mut inner = self.inner.lock();
            inner.packets.push_back(packet);
            inner.signals |= Signals::READABLE;
        }
        self.waiters.wake_one();
    }

    /// 等待事件（阻塞）
    pub fn wait(
        &self,
        packets: &mut [PortPacket],
        timeout_ns: Option<u64>,
    ) -> Result<usize, PortError> {
        let start_time = CurrentTimeArch::nano_time();

        loop {
            // 尝试获取事件
            let count = self.try_dequeue(packets);
            if count > 0 {
                return Ok(count);
            }

            if timeout_ns == Some(0) {
                return Err(PortError::WouldBlock);
            } else if let Some(timeout_ns) = timeout_ns {
                let now = CurrentTimeArch::nano_time();
                if (now - start_time) > timeout_ns {
                    return Err(PortError::WouldBlock);
                }
            }

            // 阻塞等待
            self.waiters.wait();
        }
    }

    /// 非阻塞获取事件
    pub fn try_dequeue(&self, packets: &mut [PortPacket]) -> usize {
        let mut inner = self.inner.lock();

        if inner.packets.is_empty() || packets.is_empty() {
            return 0;
        }

        let count = core::cmp::min(packets.len(), inner.packets.len());
        for i in 0..count {
            packets[i] = inner.packets.pop_front().unwrap();
        }

        // 更新信号
        if inner.packets.is_empty() {
            inner.signals.remove(Signals::READABLE);
        }

        // 清理 once 绑定
        let triggered_keys: Vec<_> = packets[..count]
            .iter()
            .filter(|p| p.packet_type == PacketType::Signal)
            .map(|p| p.key)
            .collect();

        for key in triggered_keys {
            if let Some(pos) = inner
                .bindings
                .iter()
                .position(|b| b.key == key && b.options == BindOptions::Once)
            {
                let binding = inner.bindings.remove(pos);
                binding.object.remove_signal_observer(key);
            }
        }

        count
    }

    /// 待处理事件数
    pub fn pending_count(&self) -> usize {
        self.inner.lock().packets.len()
    }
}

impl KernelObject for Port {
    fn object_type(&self) -> ObjectType {
        ObjectType::Port
    }

    fn signals(&self) -> Signals {
        self.inner.lock().signals
    }

    fn signal_set(&self, signals: Signals) {
        let mut inner = self.inner.lock();
        let old = inner.signals;
        inner.signals |= signals;
        let changed = inner.signals & !old;

        if !changed.is_empty() {
            let to_notify: Vec<_> = inner
                .observers
                .iter()
                .filter(|o| o.trigger_signals.intersects(changed))
                .map(|o| o.callback.clone())
                .collect();
            drop(inner);
            for cb in to_notify {
                cb(changed);
            }
        }
    }

    fn signal_clear(&self, signals: Signals) {
        self.inner.lock().signals.remove(signals);
    }

    fn add_signal_observer(&self, observer: SignalObserver) {
        let mut inner = self.inner.lock();

        // 如果当前信号已满足，立即触发
        let current = inner.signals;
        if current.intersects(observer.trigger_signals) {
            let cb = observer.callback.clone();
            let triggered = current & observer.trigger_signals;
            if observer.once {
                drop(inner);
                cb(triggered);
                return;
            } else {
                cb(triggered);
            }
        }

        inner.observers.push(observer);
    }

    fn remove_signal_observer(&self, key: u64) {
        self.inner.lock().observers.retain(|o| o.key != key);
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortError {
    AlreadyBound,
    NotFound,
    WouldBlock,
    InvalidArgs,
    Timeout,
}
