use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::{
    EEXIST, EWOULDBLOCK,
    object::{
        BindOptions, Channel, Handle, KernelObject, Message, Port, PortPacket, Rights, Signals,
        channel::ChannelError, port::PortError, process::current_process,
    },
};

use super::error::{EAGAIN, EBADF, EINVAL, EPERM, EPIPE, Error, Result};

/// 关闭句柄
pub fn sys_handle_close(handle: usize) -> Result<usize> {
    let handle = Handle::from(handle);

    let process = current_process().ok_or(Error::new(EINVAL))?;
    let mut proc = process.write();

    proc.handles_mut().remove(handle).ok_or(Error::new(EBADF))?;

    Ok(0)
}

/// 复制句柄
pub fn sys_handle_duplicate(handle: usize, rights: usize) -> Result<usize> {
    let handle = Handle::from(handle);
    let rights = Rights::from_bits_truncate(rights as u32);

    let process = current_process().ok_or(Error::new(EINVAL))?;
    let mut proc = process.write();

    let new_handle = proc
        .handles_mut()
        .duplicate(handle, rights)
        .ok_or(Error::new(EBADF))?;

    Ok(new_handle.raw() as usize)
}

/// 创建 Port
pub fn sys_port_create() -> Result<usize> {
    let port = Port::new();

    let process = current_process().ok_or(Error::new(EINVAL))?;
    let mut proc = process.write();

    let handle = proc.handles_mut().insert(
        port as Arc<dyn KernelObject>,
        Rights::BASIC | Rights::DUPLICATE,
    );

    Ok(handle.raw() as usize)
}

/// 等待 Port 事件
pub fn sys_port_wait(
    port_handle: usize,
    packets_ptr: usize,
    max_count: usize,
    timeout_ns: usize,
) -> Result<usize> {
    if packets_ptr == 0 || max_count == 0 {
        return Err(Error::new(EINVAL));
    }

    // 获取 Port 对象
    let port_arc = {
        let process = current_process().ok_or(Error::new(EINVAL))?;
        let proc = process.read();

        let obj = proc
            .handles()
            .get(Handle::from(port_handle), Rights::WAIT)
            .ok_or(Error::new(EBADF))?;

        // 类型检查
        if obj.as_any().downcast_ref::<Port>().is_none() {
            return Err(Error::new(EINVAL));
        }

        obj
    };

    // 获取 Port 引用进行操作
    let port = port_arc.as_any().downcast_ref::<Port>().unwrap();

    // 准备缓冲区
    let packets_slice =
        unsafe { core::slice::from_raw_parts_mut(packets_ptr as *mut PortPacket, max_count) };

    let timeout = if timeout_ns == usize::MAX {
        None
    } else {
        Some(timeout_ns as u64)
    };

    match port.wait(packets_slice, timeout) {
        Ok(count) => Ok(count),
        Err(PortError::WouldBlock) => Err(Error::new(EWOULDBLOCK)),
        Err(PortError::Timeout) => Err(Error::new(EAGAIN)),
        Err(_) => Err(Error::new(EINVAL)),
    }
}

/// 绑定对象到 Port
pub fn sys_port_bind(
    port_handle: usize,
    key: usize,
    object_handle: usize,
    signals: usize,
    options: usize,
) -> Result<usize> {
    let process = current_process().ok_or(Error::new(EINVAL))?;
    let proc = process.read();

    // 获取 Port 对象
    let port_obj = proc
        .handles()
        .get(Handle::from(port_handle), Rights::WRITE)
        .ok_or(Error::new(EBADF))?;

    // 获取要绑定的目标对象
    let target_obj = proc
        .handles()
        .get(Handle::from(object_handle), Rights::WAIT)
        .ok_or(Error::new(EBADF))?;

    drop(proc);

    // 验证 Port 类型并调用 bind
    let _port = port_obj
        .as_any()
        .downcast_ref::<Port>()
        .ok_or(Error::new(EINVAL))?;

    let port_arc = unsafe {
        // 增加引用计数，然后创建 Arc<Port>
        let ptr = Arc::as_ptr(&port_obj) as *const Port;
        Arc::increment_strong_count(ptr);
        Arc::from_raw(ptr)
    };

    port_arc
        .bind(
            key as u64,
            target_obj,
            Signals::from_bits_truncate(signals as u32),
            BindOptions::from(options as u32),
        )
        .map_err(|e| match e {
            PortError::AlreadyBound => Error::new(EEXIST),
            _ => Error::new(EINVAL),
        })?;

    Ok(0)
}

/// 解绑
pub fn sys_port_unbind(port_handle: usize, key: usize) -> Result<usize> {
    let process = current_process().ok_or(Error::new(EINVAL))?;
    let proc = process.read();

    let port_obj = proc
        .handles()
        .get(Handle::from(port_handle), Rights::WRITE)
        .ok_or(Error::new(EBADF))?;

    drop(proc);

    let port = port_obj
        .as_any()
        .downcast_ref::<Port>()
        .ok_or(Error::new(EINVAL))?;

    port.unbind(key as u64).map_err(|_| Error::new(EINVAL))?;

    Ok(0)
}

/// 手动投递事件
pub fn sys_port_queue(port_handle: usize, key: usize, data_ptr: usize) -> Result<usize> {
    let process = current_process().ok_or(Error::new(EINVAL))?;
    let proc = process.read();

    let port_obj = proc
        .handles()
        .get(Handle::from(port_handle), Rights::WRITE)
        .ok_or(Error::new(EBADF))?;

    drop(proc);

    let port = port_obj
        .as_any()
        .downcast_ref::<Port>()
        .ok_or(Error::new(EINVAL))?;

    let user_data = if data_ptr != 0 {
        unsafe { *(data_ptr as *const [u64; 4]) }
    } else {
        [0u64; 4]
    };

    let packet = PortPacket::user(key as u64, user_data);
    port.queue(packet);

    Ok(0)
}

/// 创建 Channel 对
pub fn sys_channel_create(handles_out: usize) -> Result<usize> {
    if handles_out == 0 {
        return Err(Error::new(EINVAL));
    }

    let (ch0, ch1) = Channel::create_pair();

    let process = current_process().ok_or(Error::new(EINVAL))?;
    let mut proc = process.write();

    let h0 = proc.handles_mut().insert(
        ch0 as Arc<dyn KernelObject>,
        Rights::BASIC | Rights::DUPLICATE | Rights::TRANSFER,
    );
    let h1 = proc.handles_mut().insert(
        ch1 as Arc<dyn KernelObject>,
        Rights::BASIC | Rights::DUPLICATE | Rights::TRANSFER,
    );

    // 写回句柄
    unsafe {
        let out = &mut *(handles_out as *mut [u32; 2]);
        out[0] = h0.raw();
        out[1] = h1.raw();
    }

    Ok(0)
}

/// 发送消息
pub fn sys_channel_send(
    channel_handle: usize,
    data_ptr: usize,
    data_len: usize,
    handles_ptr: usize,
    handles_count: usize,
) -> Result<usize> {
    // 准备要转移的句柄
    let handles_to_transfer: Vec<Handle> = if handles_ptr != 0 && handles_count > 0 {
        let raw_handles =
            unsafe { core::slice::from_raw_parts(handles_ptr as *const u32, handles_count) };
        raw_handles.iter().map(|&h| Handle(h)).collect()
    } else {
        Vec::new()
    };

    // 获取 Channel 并转移句柄
    let (channel_obj, transferred) = {
        let process = current_process().ok_or(Error::new(EINVAL))?;
        let mut proc = process.write();

        // 获取 Channel
        let channel_obj = proc
            .handles()
            .get(Handle::from(channel_handle), Rights::WRITE)
            .ok_or(Error::new(EBADF))?;

        // 验证 Channel 类型
        if channel_obj.as_any().downcast_ref::<Channel>().is_none() {
            return Err(Error::new(EINVAL));
        }

        // 转移句柄（从当前进程移除）
        let transferred = if !handles_to_transfer.is_empty() {
            proc.handles_mut()
                .transfer_many(&handles_to_transfer)
                .ok_or(Error::new(EPERM))?
        } else {
            Vec::new()
        };

        (channel_obj, transferred)
    };

    // 构造消息
    let data = if data_ptr != 0 && data_len > 0 {
        unsafe { core::slice::from_raw_parts(data_ptr as *const u8, data_len) }.to_vec()
    } else {
        Vec::new()
    };

    // 消息中包含对象和权限（不是句柄值）
    let msg = Message::with_objects(data, transferred);

    // 发送
    let channel = channel_obj.as_any().downcast_ref::<Channel>().unwrap();

    channel.send(msg).map_err(|e| match e {
        ChannelError::PeerClosed => Error::new(EPIPE),
        ChannelError::Full => Error::new(EAGAIN),
        _ => Error::new(EINVAL),
    })?;

    Ok(0)
}

/// 接收消息
pub fn sys_channel_recv(
    channel_handle: usize,
    data_ptr: usize,
    data_len: usize,
    handles_ptr: usize,
    handles_count: usize,
    actual_out: usize,
) -> Result<usize> {
    // 获取 Channel
    let channel_obj = {
        let process = current_process().ok_or(Error::new(EINVAL))?;
        let proc = process.read();

        proc.handles()
            .get(Handle::from(channel_handle), Rights::READ)
            .ok_or(Error::new(EBADF))?
    };

    let channel = channel_obj
        .as_any()
        .downcast_ref::<Channel>()
        .ok_or(Error::new(EINVAL))?;

    // 接收消息
    let msg = channel.recv().map_err(|e| match e {
        ChannelError::PeerClosed => Error::new(EPIPE),
        ChannelError::Empty => Error::new(EAGAIN),
        _ => Error::new(EINVAL),
    })?;

    // 复制数据
    let actual_data_len = core::cmp::min(data_len, msg.data.len());
    if data_ptr != 0 && actual_data_len > 0 {
        unsafe {
            core::ptr::copy_nonoverlapping(msg.data.as_ptr(), data_ptr as *mut u8, actual_data_len);
        }
    }

    // 将接收到的对象转换为当前进程的句柄
    let received_handles = if !msg.objects.is_empty() && handles_ptr != 0 && handles_count > 0 {
        let process = current_process().ok_or(Error::new(EINVAL))?;
        let mut proc = process.write();

        let handles: Vec<Handle> = proc.handles_mut().receive_many(msg.objects);

        // 复制句柄到用户空间
        let copy_count = core::cmp::min(handles_count, handles.len());
        unsafe {
            let out = core::slice::from_raw_parts_mut(handles_ptr as *mut u32, copy_count);
            for (i, h) in handles.iter().take(copy_count).enumerate() {
                out[i] = h.raw();
            }
        }

        handles.len()
    } else {
        0
    };

    // 写回实际长度
    if actual_out != 0 {
        unsafe {
            let out = &mut *(actual_out as *mut [usize; 2]);
            out[0] = msg.data.len();
            out[1] = received_handles;
        }
    }

    Ok(0)
}

/// 非阻塞接收
pub fn sys_channel_try_recv(
    channel_handle: usize,
    data_ptr: usize,
    data_len: usize,
    handles_ptr: usize,
    handles_count: usize,
    actual_out: usize,
) -> Result<usize> {
    // 获取 Channel
    let channel_obj = {
        let process = current_process().ok_or(Error::new(EINVAL))?;
        let proc = process.read();

        proc.handles()
            .get(Handle::from(channel_handle), Rights::READ)
            .ok_or(Error::new(EBADF))?
    };

    let channel = channel_obj
        .as_any()
        .downcast_ref::<Channel>()
        .ok_or(Error::new(EINVAL))?;

    // 非阻塞接收
    let msg = channel.try_recv().map_err(|e| match e {
        ChannelError::PeerClosed => Error::new(EPIPE),
        ChannelError::Empty => Error::new(EAGAIN),
        _ => Error::new(EINVAL),
    })?;

    // 复制数据
    let actual_data_len = core::cmp::min(data_len, msg.data.len());
    if data_ptr != 0 && actual_data_len > 0 {
        unsafe {
            core::ptr::copy_nonoverlapping(msg.data.as_ptr(), data_ptr as *mut u8, actual_data_len);
        }
    }

    // 将接收到的对象转换为当前进程的句柄
    let received_handles = if !msg.objects.is_empty() && handles_ptr != 0 && handles_count > 0 {
        let process = current_process().ok_or(Error::new(EINVAL))?;
        let mut proc = process.write();

        let handles: Vec<Handle> = proc.handles_mut().receive_many(msg.objects);

        let copy_count = core::cmp::min(handles_count, handles.len());
        unsafe {
            let out = core::slice::from_raw_parts_mut(handles_ptr as *mut u32, copy_count);
            for (i, h) in handles.iter().take(copy_count).enumerate() {
                out[i] = h.raw();
            }
        }

        handles.len()
    } else {
        0
    };

    // 写回实际长度
    if actual_out != 0 {
        unsafe {
            let out = &mut *(actual_out as *mut [usize; 2]);
            out[0] = msg.data.len();
            out[1] = received_handles;
        }
    }

    Ok(0)
}
