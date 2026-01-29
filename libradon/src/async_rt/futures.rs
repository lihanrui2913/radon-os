use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll};
use radon_kernel::{EAGAIN, Result};

use crate::channel::Channel;
use crate::port::{BindOptions, Deadline, Port, PortPacket};
use crate::signal::Signals;

/// 异步等待 Port 事件
pub struct PortWaitFuture<'a> {
    port: &'a Port,
    packets: &'a mut [PortPacket],
    deadline: Deadline,
}

impl<'a> PortWaitFuture<'a> {
    pub fn new(port: &'a Port, packets: &'a mut [PortPacket], deadline: Deadline) -> Self {
        Self {
            port,
            packets,
            deadline,
        }
    }
}

impl<'a> Future for PortWaitFuture<'a> {
    type Output = Result<usize>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // 尝试非阻塞读取
        match self.port.try_wait(self.packets) {
            Ok(count) if count > 0 => Poll::Ready(Ok(count)),
            Ok(_) => {
                // 没有事件，注册 waker 后返回 Pending
                // 实际实现中，需要将 waker 与 port 关联
                cx.waker().wake_by_ref();
                Poll::Pending
            }
            Err(e) if e.errno == EAGAIN => {
                cx.waker().wake_by_ref();
                Poll::Pending
            }
            Err(e) => Poll::Ready(Err(e)),
        }
    }
}

/// 异步接收 Channel 消息
pub struct ChannelRecvFuture<'a> {
    channel: &'a Channel,
    port: Option<&'a Port>,
    key: u64,
    buffer: &'a mut [u8],
    registered: bool,
}

impl<'a> ChannelRecvFuture<'a> {
    pub fn new(
        channel: &'a Channel,
        buffer: &'a mut [u8],
        port: Option<&'a Port>,
        key: u64,
    ) -> Self {
        Self {
            channel,
            port,
            key,
            buffer,
            registered: false,
        }
    }
}

impl<'a> Future for ChannelRecvFuture<'a> {
    type Output = Result<usize>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // 尝试非阻塞接收
        match self.channel.try_recv(self.buffer, &mut []) {
            Ok(result) => Poll::Ready(Ok(result.data_len)),
            Err(e) if e.errno == EAGAIN => {
                // 注册到 port
                if !self.registered {
                    if let Some(port) = self.port {
                        let _ = port.bind(
                            self.key,
                            self.channel,
                            Signals::READABLE | Signals::PEER_CLOSED,
                            BindOptions::Once,
                        );
                        self.registered = true;
                    }
                }
                cx.waker().wake_by_ref();
                Poll::Pending
            }
            Err(e) => Poll::Ready(Err(e)),
        }
    }
}

/// 带超时的 Future
pub struct TimeoutFuture<F> {
    future: F,
    deadline: Deadline,
    started: bool,
}

impl<F> TimeoutFuture<F> {
    pub fn new(future: F, deadline: Deadline) -> Self {
        Self {
            future,
            deadline,
            started: false,
        }
    }
}

impl<F: Future> Future for TimeoutFuture<F> {
    type Output = Result<F::Output>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // 安全：我们只 pin future 字段
        let this = unsafe { self.get_unchecked_mut() };
        let future = unsafe { Pin::new_unchecked(&mut this.future) };

        match future.poll(cx) {
            Poll::Ready(val) => Poll::Ready(Ok(val)),
            Poll::Pending => {
                // TODO: 检查超时
                // 需要定时器支持
                Poll::Pending
            }
        }
    }
}

/// 选择多个 Future 中第一个完成的
pub enum Select<A, B> {
    First(A, B),
    Done,
}

pub enum Either<A, B> {
    Left(A),
    Right(B),
}

impl<A, B> Future for Select<A, B>
where
    A: Future + Unpin,
    B: Future + Unpin,
{
    type Output = Either<A::Output, B::Output>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match &mut *self {
            Select::First(a, b) => {
                // 先尝试 poll a
                if let Poll::Ready(val) = Pin::new(a).poll(cx) {
                    *self = Select::Done;
                    return Poll::Ready(Either::Left(val));
                }
                // 再尝试 poll b
                if let Poll::Ready(val) = Pin::new(b).poll(cx) {
                    *self = Select::Done;
                    return Poll::Ready(Either::Right(val));
                }
                Poll::Pending
            }
            Select::Done => panic!("Select polled after completion"),
        }
    }
}

/// 创建 select
pub fn select<A, B>(a: A, b: B) -> Select<A, B>
where
    A: Future + Unpin,
    B: Future + Unpin,
{
    Select::First(a, b)
}

/// 让出执行权
pub struct YieldNow {
    yielded: bool,
}

impl YieldNow {
    pub fn new() -> Self {
        Self { yielded: false }
    }
}

impl Future for YieldNow {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.yielded {
            Poll::Ready(())
        } else {
            self.yielded = true;
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    }
}

/// 让出执行权
pub fn yield_now() -> YieldNow {
    YieldNow::new()
}

/// Port 的异步扩展
pub trait PortAsyncExt {
    fn wait_async<'a>(&'a self, packets: &'a mut [PortPacket]) -> PortWaitFuture<'a>;
}

impl PortAsyncExt for Port {
    fn wait_async<'a>(&'a self, packets: &'a mut [PortPacket]) -> PortWaitFuture<'a> {
        PortWaitFuture::new(self, packets, Deadline::Infinite)
    }
}

/// Channel 的异步扩展
pub trait ChannelAsyncExt {
    fn recv_async<'a>(
        &'a self,
        buffer: &'a mut [u8],
        port: &'a Port,
        key: u64,
    ) -> ChannelRecvFuture<'a>;
}

impl ChannelAsyncExt for Channel {
    fn recv_async<'a>(
        &'a self,
        buffer: &'a mut [u8],
        port: &'a Port,
        key: u64,
    ) -> ChannelRecvFuture<'a> {
        ChannelRecvFuture::new(self, buffer, Some(port), key)
    }
}
