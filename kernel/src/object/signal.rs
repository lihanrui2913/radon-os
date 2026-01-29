// kernel/src/object/signal.rs

use bitflags::bitflags;

bitflags! {
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
        /// 已触发（用于 Event/Timer）
        const SIGNALED      = 1 << 4;

        // 用户信号
        const USER_0        = 1 << 24;
        const USER_1        = 1 << 25;
        const USER_2        = 1 << 26;
        const USER_3        = 1 << 27;
    }
}
