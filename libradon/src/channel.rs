use alloc::vec::Vec;
use radon_kernel::Result;

use crate::handle::{AsHandle, Handle, OwnedHandle};
use crate::syscall::{self, nr, result_from_retval};
use core::fmt;

/// Channel 对
pub struct ChannelPair {
    pub ch0: Channel,
    pub ch1: Channel,
}

impl ChannelPair {
    /// 创建 Channel 对
    pub fn create() -> Result<Self> {
        let mut handles: [u32; 2] = [0; 2];

        let ret =
            unsafe { syscall::syscall1(nr::SYS_CHANNEL_CREATE, handles.as_mut_ptr() as usize) };
        result_from_retval(ret)?;

        Ok(Self {
            ch0: Channel::from_handle(OwnedHandle::from_raw(handles[0])),
            ch1: Channel::from_handle(OwnedHandle::from_raw(handles[1])),
        })
    }

    /// 拆分为两个独立的 Channel
    pub fn split(self) -> (Channel, Channel) {
        (self.ch0, self.ch1)
    }
}

/// Channel 对象
pub struct Channel {
    handle: OwnedHandle,
}

impl Channel {
    /// 创建 Channel 对
    pub fn create_pair() -> Result<(Channel, Channel)> {
        ChannelPair::create().map(|p| p.split())
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

    /// 发送数据（无句柄）
    pub fn send(&self, data: &[u8]) -> Result<()> {
        self.send_with_handles(data, &[])
    }

    /// 发送数据和句柄
    pub fn send_with_handles(&self, data: &[u8], handles: &[Handle]) -> Result<()> {
        let ret = unsafe {
            syscall::syscall5(
                nr::SYS_CHANNEL_SEND,
                self.handle.raw() as usize,
                data.as_ptr() as usize,
                data.len(),
                handles.as_ptr() as usize,
                handles.len(),
            )
        };
        result_from_retval(ret).map(|_| ())
    }

    /// 接收数据到缓冲区（阻塞）
    pub fn recv(&self, data: &mut [u8]) -> Result<RecvResult> {
        self.recv_with_handles(data, &mut [])
    }

    /// 接收数据和句柄
    pub fn recv_with_handles(&self, data: &mut [u8], handles: &mut [Handle]) -> Result<RecvResult> {
        let mut actual: [usize; 2] = [0; 2];

        let ret = unsafe {
            syscall::syscall6(
                nr::SYS_CHANNEL_RECV,
                self.handle.raw() as usize,
                data.as_mut_ptr() as usize,
                data.len(),
                handles.as_mut_ptr() as usize,
                handles.len(),
                actual.as_mut_ptr() as usize,
            )
        };
        result_from_retval(ret)?;

        Ok(RecvResult {
            data_len: actual[0],
            handle_count: actual[1],
        })
    }

    /// 非阻塞接收
    pub fn try_recv(&self, data: &mut [u8], handles: &mut [Handle]) -> Result<RecvResult> {
        // TODO: 添加非阻塞标志
        self.recv_with_handles(data, handles)
    }
}

impl Channel {
    /// 接收到 Vec（自动分配）
    pub fn recv_vec(&self, max_size: usize) -> Result<Vec<u8>> {
        let mut buf = alloc::vec![0u8; max_size];
        let result = self.recv(&mut buf)?;
        buf.truncate(result.data_len);
        Ok(buf)
    }

    /// 发送 Vec
    pub fn send_vec(&self, data: &Vec<u8>) -> Result<()> {
        self.send(data.as_slice())
    }
}

impl AsHandle for Channel {
    fn as_handle(&self) -> Handle {
        self.handle.handle()
    }
}

impl fmt::Debug for Channel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Channel")
            .field("handle", &self.handle.raw())
            .finish()
    }
}

/// 接收结果
#[derive(Debug, Clone, Copy)]
pub struct RecvResult {
    /// 实际数据长度
    pub data_len: usize,
    /// 实际句柄数量
    pub handle_count: usize,
}

pub mod message {
    use super::*;
    use alloc::vec::Vec;

    /// IPC 消息
    #[derive(Debug, Clone)]
    pub struct Message {
        pub data: Vec<u8>,
        pub handles: Vec<Handle>,
    }

    impl Message {
        pub fn new(data: Vec<u8>) -> Self {
            Self {
                data,
                handles: Vec::new(),
            }
        }

        pub fn with_handles(data: Vec<u8>, handles: Vec<Handle>) -> Self {
            Self { data, handles }
        }

        pub fn from_bytes(bytes: &[u8]) -> Self {
            Self {
                data: bytes.to_vec(),
                handles: Vec::new(),
            }
        }
    }

    impl Channel {
        /// 发送消息
        pub fn send_msg(&self, msg: &Message) -> Result<()> {
            self.send_with_handles(&msg.data, &msg.handles)
        }

        /// 接收消息
        pub fn recv_msg(&self, max_data: usize, max_handles: usize) -> Result<Message> {
            let mut data = alloc::vec![0u8; max_data];
            let mut handles = alloc::vec![Handle::INVALID; max_handles];

            let result = self.recv_with_handles(&mut data, &mut handles)?;

            data.truncate(result.data_len);
            handles.truncate(result.handle_count);

            Ok(Message { data, handles })
        }
    }
}
