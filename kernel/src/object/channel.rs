// kernel/src/object/channel.rs

use alloc::collections::VecDeque;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use core::any::Any;
use spin::Mutex;

use super::{KernelObject, ObjectType, Rights, SignalObserver, Signals, wait_queue::WaitQueue};

/// IPC 消息
///
/// 消息包含数据和对象引用（不是句柄值）
/// 句柄值只在特定进程上下文中有意义
#[derive(Clone)]
pub struct Message {
    /// 消息数据
    pub data: Vec<u8>,
    /// 携带的对象和权限
    pub objects: Vec<(Arc<dyn KernelObject>, Rights)>,
}

impl Message {
    /// 创建只包含数据的消息
    pub fn new(data: Vec<u8>) -> Self {
        Self {
            data,
            objects: Vec::new(),
        }
    }

    /// 从字节创建
    pub fn from_bytes(bytes: &[u8]) -> Self {
        Self {
            data: bytes.to_vec(),
            objects: Vec::new(),
        }
    }

    /// 创建包含对象的消息
    pub fn with_objects(data: Vec<u8>, objects: Vec<(Arc<dyn KernelObject>, Rights)>) -> Self {
        Self { data, objects }
    }

    /// 是否为空消息
    pub fn is_empty(&self) -> bool {
        self.data.is_empty() && self.objects.is_empty()
    }

    /// 对象数量
    pub fn object_count(&self) -> usize {
        self.objects.len()
    }
}

/// Channel 内部状态
struct ChannelInner {
    /// 消息队列
    messages: VecDeque<Message>,
    /// 对端
    peer: Option<Weak<Channel>>,
    /// 当前信号
    signals: Signals,
    /// 观察者列表
    observers: Vec<SignalObserver>,
    /// 最大队列长度
    capacity: usize,
    /// 是否已关闭
    closed: bool,
}

/// Channel 对象
pub struct Channel {
    inner: Mutex<ChannelInner>,
    waiters: WaitQueue,
}

impl Channel {
    const DEFAULT_CAPACITY: usize = 64;

    /// 创建 Channel 对
    pub fn create_pair() -> (Arc<Channel>, Arc<Channel>) {
        let ch0 = Arc::new(Channel {
            inner: Mutex::new(ChannelInner {
                messages: VecDeque::new(),
                peer: None,
                signals: Signals::WRITABLE,
                observers: Vec::new(),
                capacity: Self::DEFAULT_CAPACITY,
                closed: false,
            }),
            waiters: WaitQueue::new(),
        });

        let ch1 = Arc::new(Channel {
            inner: Mutex::new(ChannelInner {
                messages: VecDeque::new(),
                peer: None,
                signals: Signals::WRITABLE,
                observers: Vec::new(),
                capacity: Self::DEFAULT_CAPACITY,
                closed: false,
            }),
            waiters: WaitQueue::new(),
        });

        ch0.inner.lock().peer = Some(Arc::downgrade(&ch1));
        ch1.inner.lock().peer = Some(Arc::downgrade(&ch0));

        (ch0, ch1)
    }

    /// 发送消息
    pub fn send(&self, msg: Message) -> Result<(), ChannelError> {
        let peer = {
            let inner = self.inner.lock();
            if inner.closed {
                return Err(ChannelError::PeerClosed);
            }
            inner
                .peer
                .as_ref()
                .and_then(|p| p.upgrade())
                .ok_or(ChannelError::PeerClosed)?
        };

        {
            let mut peer_inner = peer.inner.lock();

            if peer_inner.closed {
                return Err(ChannelError::PeerClosed);
            }

            if peer_inner.messages.len() >= peer_inner.capacity {
                return Err(ChannelError::Full);
            }

            peer_inner.messages.push_back(msg);

            // 设置 READABLE
            let old = peer_inner.signals;
            peer_inner.signals |= Signals::READABLE;

            // 通知观察者
            if !old.contains(Signals::READABLE) {
                let to_notify: Vec<_> = peer_inner
                    .observers
                    .iter()
                    .filter(|o| o.trigger_signals.contains(Signals::READABLE))
                    .map(|o| (o.callback.clone(), o.once, o.key))
                    .collect();

                // 移除 once 观察者
                peer_inner
                    .observers
                    .retain(|o| !(o.once && o.trigger_signals.contains(Signals::READABLE)));

                drop(peer_inner);

                for (cb, _, _) in to_notify {
                    cb(Signals::READABLE);
                }
            } else {
                drop(peer_inner);
            }
        }

        // 唤醒等待接收的任务
        peer.waiters.wake_one();

        Ok(())
    }

    /// 阻塞接收
    pub fn recv(&self) -> Result<Message, ChannelError> {
        loop {
            match self.try_recv() {
                Ok(msg) => return Ok(msg),
                Err(ChannelError::Empty) => {
                    // 检查是否对端已关闭
                    {
                        let inner = self.inner.lock();
                        if inner.peer.as_ref().and_then(|p| p.upgrade()).is_none() {
                            // 但还可能有残留消息
                            if inner.messages.is_empty() {
                                return Err(ChannelError::PeerClosed);
                            }
                        }
                    }
                    // 阻塞等待
                    self.waiters.wait();
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// 非阻塞接收
    pub fn try_recv(&self) -> Result<Message, ChannelError> {
        let mut inner = self.inner.lock();

        if let Some(msg) = inner.messages.pop_front() {
            // 更新信号
            if inner.messages.is_empty() {
                inner.signals.remove(Signals::READABLE);
            }

            // 通知对端可以继续写
            if let Some(peer) = inner.peer.as_ref().and_then(|p| p.upgrade()) {
                let mut peer_inner = peer.inner.lock();
                if peer_inner.messages.len() < peer_inner.capacity {
                    peer_inner.signals |= Signals::WRITABLE;
                }
            }

            return Ok(msg);
        }

        // 检查对端
        if inner.peer.as_ref().and_then(|p| p.upgrade()).is_none() {
            return Err(ChannelError::PeerClosed);
        }

        Err(ChannelError::Empty)
    }

    /// 关闭 channel
    pub fn close(&self) {
        let mut inner = self.inner.lock();
        inner.closed = true;

        // 通知对端
        if let Some(peer) = inner.peer.take().and_then(|p| p.upgrade()) {
            let mut peer_inner = peer.inner.lock();
            peer_inner.peer = None;
            peer_inner.signals |= Signals::PEER_CLOSED;

            let to_notify: Vec<_> = peer_inner
                .observers
                .iter()
                .filter(|o| o.trigger_signals.contains(Signals::PEER_CLOSED))
                .map(|o| o.callback.clone())
                .collect();

            drop(peer_inner);

            for cb in to_notify {
                cb(Signals::PEER_CLOSED);
            }

            peer.waiters.wake_all();
        }
    }

    /// 消息队列长度
    pub fn pending_count(&self) -> usize {
        self.inner.lock().messages.len()
    }

    /// 是否有对端
    pub fn has_peer(&self) -> bool {
        self.inner
            .lock()
            .peer
            .as_ref()
            .and_then(|p| p.upgrade())
            .is_some()
    }
}

impl Drop for Channel {
    fn drop(&mut self) {
        self.close();
    }
}

impl KernelObject for Channel {
    fn object_type(&self) -> ObjectType {
        ObjectType::Channel
    }

    fn signals(&self) -> Signals {
        self.inner.lock().signals
    }

    fn signal_set(&self, signals: Signals) {
        let mut inner = self.inner.lock();
        inner.signals |= signals;
    }

    fn signal_clear(&self, signals: Signals) {
        self.inner.lock().signals.remove(signals);
    }

    fn add_signal_observer(&self, observer: SignalObserver) {
        let mut inner = self.inner.lock();
        let current = inner.signals;

        // 如果当前信号已满足，立即触发
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

/// Channel 错误
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelError {
    PeerClosed,
    Empty,
    Full,
    InvalidMessage,
}
