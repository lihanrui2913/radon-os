//! 信号定义

use bitflags::bitflags;

bitflags! {
    /// 内核对象信号
    #[repr(transparent)]
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    pub struct Signals: u32 {
        /// 可读
        const READABLE      = 1 << 0;
        /// 可写
        const WRITABLE      = 1 << 1;
        /// 对端关闭
        const PEER_CLOSED   = 1 << 2;
        /// 已终止
        const TERMINATED    = 1 << 3;
        /// 已触发
        const SIGNALED      = 1 << 4;

        // 用户自定义信号
        const USER_0        = 1 << 24;
        const USER_1        = 1 << 25;
        const USER_2        = 1 << 26;
        const USER_3        = 1 << 27;

        /// Channel 可读
        const CHANNEL_READABLE = Self::READABLE.bits();
        /// Channel 可写
        const CHANNEL_WRITABLE = Self::WRITABLE.bits();
        /// Channel 对端关闭
        const CHANNEL_PEER_CLOSED = Self::PEER_CLOSED.bits();
    }
}

impl Signals {
    #[inline]
    pub const fn none() -> Self {
        Signals::empty()
    }

    #[inline]
    pub const fn any() -> Self {
        Signals::all()
    }
}
