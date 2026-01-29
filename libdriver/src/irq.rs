//! 中断处理

use alloc::sync::Arc;
use core::sync::atomic::{AtomicBool, Ordering};

use libradon::port::{BindOptions, Deadline};

use libradon::{
    handle::Handle,
    port::{Port, PortPacket},
    signal::Signals,
};

use crate::{DriverError, Result};

/// 中断令牌
///
/// 从内核获取的中断通知对象的句柄。
pub struct IrqToken {
    handle: Handle,
    irq_number: u32,
}

impl IrqToken {
    /// 从句柄创建
    pub fn from_handle(handle: Handle, irq_number: u32) -> Self {
        Self { handle, irq_number }
    }

    /// 获取中断号
    pub fn irq_number(&self) -> u32 {
        self.irq_number
    }

    /// 获取句柄
    pub fn handle(&self) -> Handle {
        self.handle
    }
}

/// 中断处理器
pub struct IrqHandler {
    token: IrqToken,
    port: Port,
    key: u64,
    running: Arc<AtomicBool>,
}

impl IrqHandler {
    /// 创建中断处理器
    pub fn new(token: IrqToken) -> Result<Self> {
        let port = Port::create()?;
        let key = 1;

        // 绑定中断令牌到 port
        port.bind(
            key,
            &token.handle, // 需要 Handle 实现 AsHandle
            Signals::SIGNALED,
            BindOptions::Persistent,
        )?;

        Ok(Self {
            token,
            port,
            key,
            running: Arc::new(AtomicBool::new(false)),
        })
    }

    /// 等待中断
    pub fn wait(&self) -> Result<()> {
        self.wait_timeout(Deadline::Infinite)
    }

    /// 带超时等待中断
    pub fn wait_timeout(&self, deadline: Deadline) -> Result<()> {
        let mut packets = [PortPacket::zeroed(); 1];

        let count = self.port.wait(&mut packets, deadline)?;

        if count == 0 {
            return Err(DriverError::Timeout);
        }

        if packets[0].signals.contains(Signals::SIGNALED) {
            Ok(())
        } else {
            Err(DriverError::IoError)
        }
    }

    /// 确认中断
    pub fn ack(&self) -> Result<()> {
        // TODO: 调用 SYS_IRQ_ACK syscall
        // unsafe { syscall1(SYS_IRQ_ACK, self.token.handle.raw() as usize) }
        Ok(())
    }

    /// 运行中断处理循环
    pub fn run<F>(&self, mut handler: F) -> Result<()>
    where
        F: FnMut() -> bool, // 返回 false 停止循环
    {
        self.running.store(true, Ordering::SeqCst);

        while self.running.load(Ordering::SeqCst) {
            self.wait()?;

            if !handler() {
                break;
            }

            self.ack()?;
        }

        Ok(())
    }

    /// 停止处理循环
    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
        // 发送虚假事件唤醒
        let _ = self.port.queue_user(0, [0; 4]);
    }

    /// 获取运行状态
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }
}

/// 中断处理器构建器
pub struct IrqHandlerBuilder {
    token: IrqToken,
}

impl IrqHandlerBuilder {
    pub fn new(token: IrqToken) -> Self {
        Self { token }
    }

    pub fn build(self) -> Result<IrqHandler> {
        IrqHandler::new(self.token)
    }
}
