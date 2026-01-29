use radon_kernel::{EINVAL, EWOULDBLOCK, Error, Result};

use crate::handle::{AsHandle, Handle, OwnedHandle};
use crate::signal::Signals;
use crate::syscall::{self, nr, result_from_retval};
use core::fmt;

/// 事件包
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PortPacket {
    /// 用户定义的 key
    pub key: u64,
    /// 触发的信号
    pub signals: Signals,
    /// 包类型
    pub packet_type: PacketType,
    /// 保留
    pub reserved: u32,
    /// 用户数据
    pub data: [u64; 4],
}

impl PortPacket {
    /// 创建空包
    #[inline]
    pub const fn zeroed() -> Self {
        Self {
            key: 0,
            signals: Signals::empty(),
            packet_type: PacketType::User,
            reserved: 0,
            data: [0; 4],
        }
    }

    /// 创建用户包
    #[inline]
    pub const fn user(key: u64, data: [u64; 4]) -> Self {
        Self {
            key,
            signals: Signals::empty(),
            packet_type: PacketType::User,
            reserved: 0,
            data,
        }
    }

    /// 是否为信号包
    #[inline]
    pub const fn is_signal(&self) -> bool {
        matches!(self.packet_type, PacketType::Signal)
    }

    /// 是否为用户包
    #[inline]
    pub const fn is_user(&self) -> bool {
        matches!(self.packet_type, PacketType::User)
    }
}

impl Default for PortPacket {
    fn default() -> Self {
        Self::zeroed()
    }
}

impl fmt::Debug for PortPacket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PortPacket")
            .field("key", &self.key)
            .field("signals", &self.signals)
            .field("packet_type", &self.packet_type)
            .finish()
    }
}

/// 包类型
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacketType {
    Signal = 0,
    User = 1,
    Timer = 2,
    Interrupt = 3,
}

/// 绑定选项
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindOptions {
    /// 触发一次后自动解绑
    Once = 0,
    /// 持续触发
    Persistent = 1,
}

/// 超时时间
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Deadline {
    /// 立即返回（非阻塞）
    Immediate,
    /// 无限等待
    Infinite,
    /// 绝对时间（纳秒）
    Absolute(u64),
    /// 相对时间（纳秒）
    Relative(u64),
}

impl Deadline {
    /// 转换为系统调用参数
    pub fn to_timeout_ns(&self) -> u64 {
        match self {
            Deadline::Immediate => 0,
            Deadline::Infinite => u64::MAX,
            Deadline::Absolute(t) => *t,
            Deadline::Relative(t) => {
                // TODO: 获取当前时间并计算
                *t
            }
        }
    }
}

/// Port 对象
pub struct Port {
    handle: OwnedHandle,
}

impl Port {
    /// 创建新的 Port
    pub fn create() -> Result<Self> {
        let ret = unsafe { syscall::syscall0(nr::SYS_PORT_CREATE) };
        let handle = result_from_retval(ret)? as u32;

        Ok(Self {
            handle: OwnedHandle::from_raw(handle),
        })
    }

    /// 从现有句柄创建
    #[inline]
    pub const fn from_handle(handle: OwnedHandle) -> Self {
        Self { handle }
    }

    /// 获取句柄
    #[inline]
    pub fn handle(&self) -> Handle {
        self.handle.handle()
    }

    /// 获取原始句柄值
    #[inline]
    pub fn raw(&self) -> u32 {
        self.handle.raw()
    }

    /// 绑定对象到端口
    pub fn bind<T: AsHandle>(
        &self,
        key: u64,
        object: &T,
        signals: Signals,
        options: BindOptions,
    ) -> Result<()> {
        let ret = unsafe {
            syscall::syscall5(
                nr::SYS_PORT_BIND,
                self.handle.raw() as usize,
                key as usize,
                object.as_handle().raw() as usize,
                signals.bits() as usize,
                options as usize,
            )
        };
        result_from_retval(ret).map(|_| ())
    }

    /// 解除绑定
    pub fn unbind(&self, key: u64) -> Result<()> {
        let ret = unsafe {
            syscall::syscall2(
                nr::SYS_PORT_UNBIND,
                self.handle.raw() as usize,
                key as usize,
            )
        };
        result_from_retval(ret).map(|_| ())
    }

    /// 等待事件
    pub fn wait(&self, packets: &mut [PortPacket], deadline: Deadline) -> Result<usize> {
        if packets.is_empty() {
            return Err(Error::new(EINVAL));
        }

        let ret = unsafe {
            syscall::syscall4(
                nr::SYS_PORT_WAIT,
                self.handle.raw() as usize,
                packets.as_mut_ptr() as usize,
                packets.len(),
                deadline.to_timeout_ns() as usize,
            )
        };
        result_from_retval(ret)
    }

    /// 非阻塞等待
    #[inline]
    pub fn try_wait(&self, packets: &mut [PortPacket]) -> Result<usize> {
        self.wait(packets, Deadline::Immediate)
    }

    /// 阻塞等待（无超时）
    #[inline]
    pub fn wait_blocking(&self, packets: &mut [PortPacket]) -> Result<usize> {
        self.wait(packets, Deadline::Infinite)
    }

    /// 等待单个事件
    pub fn wait_one(&self, deadline: Deadline) -> Result<PortPacket> {
        let mut packets = [PortPacket::zeroed(); 1];
        let count = self.wait(&mut packets, deadline)?;
        if count > 0 {
            Ok(packets[0])
        } else {
            Err(Error::new(EWOULDBLOCK))
        }
    }

    /// 手动投递事件
    pub fn queue(&self, packet: &PortPacket) -> Result<()> {
        let ret = unsafe {
            syscall::syscall3(
                nr::SYS_PORT_QUEUE,
                self.handle.raw() as usize,
                packet.key as usize,
                packet.data.as_ptr() as usize,
            )
        };
        result_from_retval(ret).map(|_| ())
    }

    /// 投递用户事件
    pub fn queue_user(&self, key: u64, data: [u64; 4]) -> Result<()> {
        let packet = PortPacket::user(key, data);
        self.queue(&packet)
    }
}

impl AsHandle for Port {
    fn as_handle(&self) -> Handle {
        self.handle.handle()
    }
}

impl fmt::Debug for Port {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Port")
            .field("handle", &self.handle.raw())
            .finish()
    }
}

pub struct WaitManyResult<'a> {
    pub packets: &'a [PortPacket],
    pub count: usize,
}

pub fn wait_many(port: &Port, packets: &mut [PortPacket], deadline: Deadline) -> Result<usize> {
    port.wait(packets, deadline)
}
