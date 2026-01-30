//! NVMe 驱动实现

use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU16, Ordering};
use radon_kernel::{EINVAL, EIO, ENOMEM, ETIMEDOUT, Error, Result};
use spin::{Mutex, RwLock};

use libdriver::dma::{DmaRegion, PhysAddr};
use libdriver::mmio::MmioRegion;

use crate::nvme::regs::{ControllerCapabilities, NvmeRegs};

mod regs;
pub use self::regs::{aqa, cc, csts};

/// 命令操作码
mod opcode {
    // Admin 命令
    pub const ADMIN_DELETE_SQ: u8 = 0x00;
    pub const ADMIN_CREATE_SQ: u8 = 0x01;
    pub const ADMIN_DELETE_CQ: u8 = 0x04;
    pub const ADMIN_CREATE_CQ: u8 = 0x05;
    pub const ADMIN_IDENTIFY: u8 = 0x06;
    pub const ADMIN_SET_FEATURES: u8 = 0x09;

    // I/O 命令
    pub const IO_FLUSH: u8 = 0x00;
    pub const IO_WRITE: u8 = 0x01;
    pub const IO_READ: u8 = 0x02;
}

const PAGE_SIZE: usize = 4096;
const DEFAULT_QUEUE_DEPTH: u16 = 64;
const SUBMISSION_ENTRY_SIZE: usize = 64;
const COMPLETION_ENTRY_SIZE: usize = 16;

/// NVMe 提交队列条目 (Submission Queue Entry)
#[derive(Clone, Copy, Debug, Default)]
#[repr(C, packed)]
pub struct SubmissionEntry {
    /// Opcode (CDW0[7:0])
    pub opcode: u8,
    /// Flags (CDW0[15:8]) - FUSE[1:0], reserved[5:2], PSDT[7:6]
    pub flags: u8,
    /// Command ID (CDW0[31:16])
    pub cid: u16,
    /// Namespace ID (CDW1)
    pub nsid: u32,
    /// Reserved (CDW2-3)
    pub _rsvd: u64,
    /// Metadata pointer (CDW4-5)
    pub mptr: u64,
    /// Data pointer - PRP1 or SGL (CDW6-7)
    pub dptr1: u64,
    /// Data pointer - PRP2 or SGL (CDW8-9)
    pub dptr2: u64,
    /// Command dword 10
    pub cdw10: u32,
    /// Command dword 11
    pub cdw11: u32,
    /// Command dword 12
    pub cdw12: u32,
    /// Command dword 13
    pub cdw13: u32,
    /// Command dword 14
    pub cdw14: u32,
    /// Command dword 15
    pub cdw15: u32,
}

impl SubmissionEntry {
    /// 创建 Identify 命令
    pub fn identify(cid: u16, nsid: u32, cns: u8, prp1: u64) -> Self {
        Self {
            opcode: opcode::ADMIN_IDENTIFY,
            cid,
            nsid,
            dptr1: prp1,
            cdw10: cns as u32,
            ..Default::default()
        }
    }

    /// 创建 Create I/O Completion Queue 命令
    pub fn create_io_cq(cid: u16, qid: u16, prp: u64, size: u16) -> Self {
        Self {
            opcode: opcode::ADMIN_CREATE_CQ,
            cid,
            dptr1: prp,
            cdw10: ((size as u32 - 1) << 16) | (qid as u32),
            cdw11: 1, // Physically Contiguous, Interrupts Enabled
            ..Default::default()
        }
    }

    /// 创建 Create I/O Submission Queue 命令
    pub fn create_io_sq(cid: u16, qid: u16, prp: u64, size: u16, cqid: u16) -> Self {
        Self {
            opcode: opcode::ADMIN_CREATE_SQ,
            cid,
            dptr1: prp,
            cdw10: ((size as u32 - 1) << 16) | (qid as u32),
            cdw11: ((cqid as u32) << 16) | 1, // Physically Contiguous
            ..Default::default()
        }
    }

    /// 创建 Delete I/O Submission Queue 命令
    pub fn delete_io_sq(cid: u16, qid: u16) -> Self {
        Self {
            opcode: opcode::ADMIN_DELETE_SQ,
            cid,
            cdw10: qid as u32,
            ..Default::default()
        }
    }

    /// 创建 Delete I/O Completion Queue 命令
    pub fn delete_io_cq(cid: u16, qid: u16) -> Self {
        Self {
            opcode: opcode::ADMIN_DELETE_CQ,
            cid,
            cdw10: qid as u32,
            ..Default::default()
        }
    }

    /// 创建 Read 命令
    pub fn read(cid: u16, nsid: u32, lba: u64, block_count: u16, prp1: u64, prp2: u64) -> Self {
        Self {
            opcode: opcode::IO_READ,
            cid,
            nsid,
            dptr1: prp1,
            dptr2: prp2,
            cdw10: lba as u32,
            cdw11: (lba >> 32) as u32,
            cdw12: (block_count - 1) as u32, // 0-based
            ..Default::default()
        }
    }

    /// 创建 Write 命令
    pub fn write(cid: u16, nsid: u32, lba: u64, block_count: u16, prp1: u64, prp2: u64) -> Self {
        Self {
            opcode: opcode::IO_WRITE,
            cid,
            nsid,
            dptr1: prp1,
            dptr2: prp2,
            cdw10: lba as u32,
            cdw11: (lba >> 32) as u32,
            cdw12: (block_count - 1) as u32,
            ..Default::default()
        }
    }

    /// 创建 Flush 命令
    pub fn flush(cid: u16, nsid: u32) -> Self {
        Self {
            opcode: opcode::IO_FLUSH,
            cid,
            nsid,
            ..Default::default()
        }
    }
}

/// NVMe 完成队列条目 (Completion Queue Entry)
#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct CompletionEntry {
    /// Command specific (DW0)
    pub dw0: u32,
    /// Reserved (DW1)
    pub dw1: u32,
    /// SQ Head Pointer (DW2[15:0]), SQ ID (DW2[31:16])
    pub sq_head_sqid: u32,
    /// Command ID (DW3[15:0]), Phase Tag (DW3[16]), Status (DW3[31:17])
    pub cid_status: u32,
}

impl CompletionEntry {
    /// 获取命令 ID
    #[inline]
    pub fn cid(&self) -> u16 {
        (self.cid_status & 0xFFFF) as u16
    }

    /// 获取 Phase Tag
    #[inline]
    pub fn phase(&self) -> bool {
        (self.cid_status >> 16) & 1 == 1
    }

    /// 获取状态码
    #[inline]
    pub fn status(&self) -> u16 {
        ((self.cid_status >> 17) & 0x7FFF) as u16
    }

    /// 检查是否成功
    #[inline]
    pub fn is_success(&self) -> bool {
        self.status() == 0
    }

    /// 获取 SQ Head Pointer
    #[inline]
    pub fn sq_head(&self) -> u16 {
        (self.sq_head_sqid & 0xFFFF) as u16
    }

    /// 获取 SQ ID
    #[inline]
    pub fn sq_id(&self) -> u16 {
        ((self.sq_head_sqid >> 16) & 0xFFFF) as u16
    }
}

/// PRP 条目类型
#[derive(Debug)]
pub enum PrpMode {
    /// 数据在单个页内，只需要 PRP1
    Single,
    /// 数据跨两个页，使用 PRP1 和 PRP2
    Double,
    /// 数据跨多个页，PRP2 指向 PRP List
    List,
}

/// PRP 构建器
///
/// 用于为 I/O 操作构建 PRP 条目
pub struct PrpBuilder {
    /// PRP List 的 DMA 区域（如果需要）
    prp_list: Option<DmaRegion>,
    /// PRP1 值
    prp1: u64,
    /// PRP2 值
    prp2: u64,
}

impl PrpBuilder {
    /// 从 DMA 区域构建 PRP
    ///
    /// # 参数
    /// - `region`: DMA 区域
    /// - `offset`: 区域内偏移
    /// - `length`: 数据长度
    pub fn new(region: &DmaRegion, offset: usize, length: usize) -> Result<Self> {
        let base_phys = region.phys_addr().as_u64() + offset as u64;
        Self::from_phys(base_phys, length)
    }

    /// 从物理地址构建 PRP
    pub fn from_phys(base_phys: u64, length: usize) -> Result<Self> {
        if length == 0 {
            return Ok(Self {
                prp_list: None,
                prp1: 0,
                prp2: 0,
            });
        }

        // 计算起始页偏移
        let page_offset = (base_phys as usize) & (PAGE_SIZE - 1);

        // 计算第一个 PRP 能覆盖的长度
        let first_prp_len = core::cmp::min(PAGE_SIZE - page_offset, length);

        // 剩余长度
        let remaining = length - first_prp_len;

        // PRP1 总是指向数据起始物理地址
        let prp1 = base_phys;

        if remaining == 0 {
            // 单个 PRP 就够了
            Ok(Self {
                prp_list: None,
                prp1,
                prp2: 0,
            })
        } else if remaining <= PAGE_SIZE {
            let prp2 = base_phys + first_prp_len as u64;
            Ok(Self {
                prp_list: None,
                prp1,
                prp2,
            })
        } else {
            // 需要 PRP List
            Self::build_prp_list(prp1, first_prp_len, base_phys, length)
        }
    }

    /// 构建 PRP List
    fn build_prp_list(
        prp1: u64,
        first_prp_len: usize,
        base_phys: u64,
        total_length: usize,
    ) -> Result<Self> {
        let remaining = total_length - first_prp_len;

        // 计算需要多少个 PRP 条目（除了 PRP1）
        let num_prps = (remaining + PAGE_SIZE - 1) / PAGE_SIZE;

        // 每个 PRP 条目 8 字节
        let prp_list_size = num_prps * 8;

        // 分配 PRP List
        let prp_list_region = DmaRegion::allocate_aligned(prp_list_size, PAGE_SIZE)
            .map_err(|_| Error::new(ENOMEM))?;

        // 填充 PRP List
        let prp_list_ptr = prp_list_region.virt_addr() as *mut u64;
        let mut current_phys = base_phys + first_prp_len as u64;

        for i in 0..num_prps {
            unsafe {
                prp_list_ptr.add(i).write_volatile(current_phys);
            }
            current_phys += PAGE_SIZE as u64;
        }

        Ok(Self {
            prp1,
            prp2: prp_list_region.phys_addr().as_u64(),
            prp_list: Some(prp_list_region),
        })
    }

    /// 从多个物理地址段构建 PRP
    pub fn from_segments(segments: &[(PhysAddr, usize)]) -> Result<Self> {
        if segments.is_empty() {
            return Ok(Self {
                prp_list: None,
                prp1: 0,
                prp2: 0,
            });
        }

        // 计算总长度和 PRP 条目
        let mut prp_entries: Vec<u64> = Vec::new();

        for (phys, len) in segments {
            let mut offset = 0usize;
            while offset < *len {
                let page_offset = ((phys.as_u64() + offset as u64) as usize) & (PAGE_SIZE - 1);
                let chunk = core::cmp::min(PAGE_SIZE - page_offset, len - offset);
                prp_entries.push(phys.as_u64() + offset as u64);
                offset += chunk;
            }
        }

        if prp_entries.is_empty() {
            return Err(Error::new(EINVAL));
        }

        let prp1 = prp_entries[0];

        if prp_entries.len() == 1 {
            Ok(Self {
                prp_list: None,
                prp1,
                prp2: 0,
            })
        } else if prp_entries.len() == 2 {
            Ok(Self {
                prp_list: None,
                prp1,
                prp2: prp_entries[1],
            })
        } else {
            // 需要 PRP List
            let num_prps = prp_entries.len() - 1;
            let prp_list_size = num_prps * 8;
            let prp_list_region = DmaRegion::allocate_aligned(prp_list_size, PAGE_SIZE)
                .map_err(|_| Error::new(ENOMEM))?;

            let prp_list_ptr = prp_list_region.virt_addr() as *mut u64;
            for (i, &phys) in prp_entries[1..].iter().enumerate() {
                unsafe {
                    prp_list_ptr.add(i).write_volatile(phys);
                }
            }

            Ok(Self {
                prp1,
                prp2: prp_list_region.phys_addr().as_u64(),
                prp_list: Some(prp_list_region),
            })
        }
    }

    /// 获取 PRP1
    #[inline]
    pub fn prp1(&self) -> u64 {
        self.prp1
    }

    /// 获取 PRP2
    #[inline]
    pub fn prp2(&self) -> u64 {
        self.prp2
    }

    /// 获取 PRP 模式
    pub fn mode(&self) -> PrpMode {
        if self.prp_list.is_some() {
            PrpMode::List
        } else if self.prp2 != 0 {
            PrpMode::Double
        } else {
            PrpMode::Single
        }
    }
}

/// 队列状态
struct QueueState {
    /// 当前 tail（提交队列用）
    tail: u16,
    /// 当前 head（完成队列用）
    head: u16,
    /// Phase bit（完成队列用）
    phase: bool,
}

/// NVMe 提交队列
pub struct SubmissionQueue {
    /// DMA 区域
    region: DmaRegion,
    /// 队列深度
    depth: u16,
    /// 状态
    state: Mutex<QueueState>,
}

impl SubmissionQueue {
    /// 创建提交队列
    pub fn new(depth: u16) -> Result<Self> {
        let size = depth as usize * SUBMISSION_ENTRY_SIZE;
        let mut region =
            DmaRegion::allocate_aligned(size, PAGE_SIZE).map_err(|_| Error::new(ENOMEM))?;
        region.zero();

        Ok(Self {
            region,
            depth,
            state: Mutex::new(QueueState {
                tail: 0,
                head: 0,
                phase: false,
            }),
        })
    }

    /// 获取物理地址
    pub fn phys_addr(&self) -> PhysAddr {
        self.region.phys_addr()
    }

    /// 提交命令，返回命令槽位
    pub fn submit(&self, entry: &SubmissionEntry) -> Option<u16> {
        let mut state = self.state.lock();
        let tail = state.tail;

        let next_tail = (tail + 1) % self.depth;
        if next_tail == state.head {
            return None;
        }

        // 写入命令
        let entry_ptr =
            unsafe { (self.region.virt_addr() as *mut SubmissionEntry).add(tail as usize) };
        unsafe {
            core::ptr::write_volatile(entry_ptr, *entry);
        }

        state.tail = next_tail;
        Some(tail)
    }

    /// 获取当前 tail
    pub fn tail(&self) -> u16 {
        self.state.lock().tail
    }

    /// 更新 head（从完成队列获知）
    pub fn update_head(&self, head: u16) {
        self.state.lock().head = head;
    }
}

/// NVMe 完成队列
pub struct CompletionQueue {
    /// DMA 区域
    region: DmaRegion,
    /// 队列深度
    depth: u16,
    /// 状态
    state: Mutex<QueueState>,
}

impl CompletionQueue {
    /// 创建完成队列
    pub fn new(depth: u16) -> Result<Self> {
        let size = depth as usize * COMPLETION_ENTRY_SIZE;
        let mut region =
            DmaRegion::allocate_aligned(size, PAGE_SIZE).map_err(|_| Error::new(ENOMEM))?;

        region.zero();

        Ok(Self {
            region,
            depth,
            state: Mutex::new(QueueState {
                tail: 0,
                head: 0,
                phase: true, // 初始 phase 为 1
            }),
        })
    }

    /// 获取物理地址
    pub fn phys_addr(&self) -> PhysAddr {
        self.region.phys_addr()
    }

    /// 尝试获取下一个完成条目
    pub fn poll(&self) -> Option<CompletionEntry> {
        let mut state = self.state.lock();
        let head = state.head;

        let entry_ptr =
            unsafe { (self.region.virt_addr() as *const CompletionEntry).add(head as usize) };
        let entry = unsafe { core::ptr::read_volatile(entry_ptr) };

        // 检查 phase bit
        if entry.phase() != state.phase {
            return None;
        }

        // 更新 head
        state.head = (head + 1) % self.depth;
        if state.head == 0 {
            state.phase = !state.phase;
        }

        Some(entry)
    }

    /// 获取当前 head
    pub fn head(&self) -> u16 {
        self.state.lock().head
    }
}

/// 命令 ID 分配器
pub struct CommandIdAllocator {
    next: AtomicU16,
    max: u16,
}

impl CommandIdAllocator {
    pub fn new(max: u16) -> Self {
        Self {
            next: AtomicU16::new(0),
            max,
        }
    }

    pub fn allocate(&self) -> u16 {
        loop {
            let current = self.next.load(Ordering::Relaxed);
            let next = (current + 1) % self.max;
            if self
                .next
                .compare_exchange_weak(current, next, Ordering::SeqCst, Ordering::Relaxed)
                .is_ok()
            {
                return current;
            }
        }
    }
}

/// 待处理命令
struct PendingCommand {
    /// PRP 构建器（保持 PRP list 内存不被 drop ）
    prp: Option<PrpBuilder>,
    /// 数据缓冲区引用
    buffer: Option<Arc<DmaRegion>>,
}

/// NVMe 队列对
pub struct QueuePair {
    /// 队列 ID
    pub id: u16,
    /// 提交队列
    sq: SubmissionQueue,
    /// 完成队列
    cq: CompletionQueue,
    /// 命令 ID 分配器
    cid_alloc: CommandIdAllocator,
    /// 待处理命令
    pending: Mutex<BTreeMap<u16, PendingCommand>>,
    /// 门铃步长（缓存）
    doorbell_stride: usize,
}

impl QueuePair {
    /// 创建队列对
    pub fn new(id: u16, depth: u16, doorbell_stride: usize) -> Result<Self> {
        Ok(Self {
            id,
            sq: SubmissionQueue::new(depth)?,
            cq: CompletionQueue::new(depth)?,
            cid_alloc: CommandIdAllocator::new(depth),
            pending: Mutex::new(BTreeMap::new()),
            doorbell_stride,
        })
    }

    /// 提交命令（使用 NvmeRegs）
    pub fn submit(
        &self,
        regs: &NvmeRegs,
        mut entry: SubmissionEntry,
        prp: Option<PrpBuilder>,
        buffer: Option<Arc<DmaRegion>>,
    ) -> Result<u16> {
        let cid = self.cid_alloc.allocate();
        entry.cid = cid;

        self.pending
            .lock()
            .insert(cid, PendingCommand { prp, buffer });
        self.sq.submit(&entry);

        regs.write_sq_doorbell(self.id, self.doorbell_stride, self.sq.tail());

        Ok(cid)
    }

    pub fn submit_entry(&self, mut entry: SubmissionEntry) -> Result<u16> {
        let cid = self.cid_alloc.allocate();
        entry.cid = cid;
        self.sq.submit(&entry);

        Ok(cid)
    }

    /// 轮询完成（使用 NvmeRegs）
    pub fn poll_completion(&self, regs: &NvmeRegs) -> Option<CompletionEntry> {
        if let Some(entry) = self.cq.poll() {
            self.sq.update_head(entry.sq_head());
            self.pending.lock().remove(&entry.cid());

            // 写 CQ 门铃
            regs.write_cq_doorbell(self.id, self.doorbell_stride, self.cq.head());

            Some(entry)
        } else {
            None
        }
    }

    /// 等待指定命令完成
    pub fn wait_completion(&self, regs: &NvmeRegs, cid: u16) -> Result<CompletionEntry> {
        loop {
            if let Some(entry) = self.poll_completion(regs) {
                if entry.cid() == cid {
                    if entry.is_success() {
                        return Ok(entry);
                    } else {
                        return Err(Error::new(EIO));
                    }
                }
            }
            core::hint::spin_loop();
        }
    }

    pub fn sq_phys(&self) -> PhysAddr {
        self.sq.phys_addr()
    }
    pub fn cq_phys(&self) -> PhysAddr {
        self.cq.phys_addr()
    }
    pub fn depth(&self) -> u16 {
        self.sq.depth
    }
    pub fn sq_tail(&self) -> u16 {
        self.sq.tail()
    }
    pub fn cq_head(&self) -> u16 {
        self.cq.head()
    }
}

/// 控制器信息 (Identify Controller)
#[derive(Debug, Clone)]
pub struct ControllerInfo {
    /// 供应商 ID
    pub vendor_id: u16,
    /// 序列号
    pub serial_number: [u8; 20],
    /// 模型号
    pub model_number: [u8; 40],
    /// 固件版本
    pub firmware_revision: [u8; 8],
    /// 最大数据传输大小 (2^MDTS * 最小页大小)，0 表示无限制
    pub max_transfer_size: Option<usize>,
    /// 命名空间数量
    pub nn: u32,
}

/// Namespace 信息 (Identify Namespace)
#[derive(Debug, Clone)]
pub struct NamespaceInfo {
    /// Namespace ID
    pub nsid: u32,
    /// 容量 (逻辑块数)
    pub size: u64,
    /// 容量 (字节)
    pub capacity: u64,
    /// 逻辑块大小
    pub block_size: u32,
    /// 格式化 LBA 大小索引
    pub formatted_lba_size: u8,
}

/// NVMe 控制器
pub struct NvmeController {
    /// 寄存器访问
    regs: NvmeRegs,
    /// 控制器能力
    capabilities: ControllerCapabilities,
    /// 控制器信息
    info: Option<ControllerInfo>,
    /// Admin 队列对
    admin_queue: QueuePair,
    /// I/O 队列对
    io_queues: RwLock<Vec<Arc<QueuePair>>>,
    /// Namespace 列表
    namespaces: RwLock<BTreeMap<u32, Arc<NvmeNamespace>>>,
    /// 下一个 I/O 队列 ID
    next_io_qid: AtomicU16,
}

impl NvmeController {
    /// 创建新的 NVMe 控制器
    pub unsafe fn new(bar0_phys: PhysAddr, bar0_size: usize) -> Result<Arc<Self>> {
        // 映射 MMIO
        let mmio = MmioRegion::map(bar0_phys, bar0_size).map_err(|_| Error::new(ENOMEM))?;
        let regs = NvmeRegs::new(mmio);

        // 读取能力 - 使用宏生成的方法
        let capabilities = regs.read_capabilities();

        // 创建 Admin 队列
        let admin_depth = core::cmp::min(64, capabilities.max_queue_entries);
        let admin_queue = QueuePair::new(0, admin_depth, capabilities.doorbell_stride)?;

        let controller = Arc::new(Self {
            regs,
            capabilities,
            info: None,
            admin_queue,
            io_queues: RwLock::new(Vec::new()),
            namespaces: RwLock::new(BTreeMap::new()),
            next_io_qid: AtomicU16::new(1),
        });

        controller.init()?;
        Ok(controller)
    }

    /// 初始化控制器
    fn init(&self) -> Result<()> {
        self.disable()?;
        self.configure_admin_queues()?;
        self.enable()?;
        self.identify_controller()?;
        Ok(())
    }

    /// 禁用控制器
    fn disable(&self) -> Result<()> {
        // 使用宏生成的 cc() 方法
        let cc_val = self.regs.cc().read();
        if cc_val & cc::EN != 0 {
            self.regs.cc().write(cc_val & !cc::EN);

            // 使用辅助方法等待禁用
            self.regs
                .wait_disabled(self.capabilities.timeout_ms)
                .map_err(|_| Error::new(ETIMEDOUT))?;
        }
        Ok(())
    }

    /// 启用控制器
    fn enable(&self) -> Result<()> {
        // 使用辅助函数构建 CC 值
        // MPS = 0 (4KB), IOSQES = 6 (64 bytes), IOCQES = 4 (16 bytes)
        let cc_val = cc::build(true, 0, 6, 4);

        // 使用宏生成的方法写入
        self.regs.cc().write(cc_val);

        // 等待就绪
        self.regs
            .wait_ready(self.capabilities.timeout_ms)
            .map_err(|_| Error::new(ETIMEDOUT))?;

        // 检查是否有错误
        if csts::is_fatal(self.regs.csts().read()) {
            return Err(Error::new(EIO));
        }

        Ok(())
    }

    /// 配置 Admin 队列
    fn configure_admin_queues(&self) -> Result<()> {
        let depth = self.admin_queue.depth();

        // 使用宏生成的方法 + 辅助函数
        self.regs.aqa().write(aqa::build(depth, depth));
        self.regs.asq().write(self.admin_queue.sq_phys().as_u64());
        self.regs.acq().write(self.admin_queue.cq_phys().as_u64());

        Ok(())
    }

    /// 识别控制器
    fn identify_controller(&self) -> Result<()> {
        let buffer = DmaRegion::allocate(4096).map_err(|_| Error::new(ENOMEM))?;

        let entry = SubmissionEntry::identify(
            0,
            0,
            1, // CNS = 1 for Controller
            buffer.phys_addr().as_u64(),
        );

        let cid = self.submit_admin_cmd(entry)?;
        self.wait_admin_completion(cid)?;

        // TODO: 解析控制器信息
        Ok(())
    }

    /// 提交 Admin 命令
    fn submit_admin_cmd(&self, entry: SubmissionEntry) -> Result<u16> {
        let cid = self.admin_queue.submit_entry(entry)?;

        // 使用扩展方法写门铃
        self.regs.write_sq_doorbell(
            0,
            self.capabilities.doorbell_stride,
            self.admin_queue.sq_tail(),
        );

        Ok(cid)
    }

    /// 等待 Admin 命令完成
    fn wait_admin_completion(&self, cid: u16) -> Result<CompletionEntry> {
        loop {
            if let Some(entry) = self.admin_queue.poll_completion(self.regs()) {
                // 写 CQ 门铃
                self.regs.write_cq_doorbell(
                    0,
                    self.capabilities.doorbell_stride,
                    self.admin_queue.cq_head(),
                );

                if entry.cid() == cid {
                    if entry.is_success() {
                        return Ok(entry);
                    } else {
                        return Err(Error::new(EIO));
                    }
                }
            }
            core::hint::spin_loop();
        }
    }

    /// 创建 I/O 队列对
    pub fn create_io_queue(&self) -> Result<Arc<QueuePair>> {
        let qid = self.next_io_qid.fetch_add(1, Ordering::SeqCst);
        let depth = core::cmp::min(64, self.capabilities.max_queue_entries);

        let queue_pair = Arc::new(QueuePair::new(
            qid,
            depth,
            self.capabilities.doorbell_stride,
        )?);

        // 创建 CQ
        let create_cq = SubmissionEntry::create_io_cq(0, qid, queue_pair.cq_phys().as_u64(), depth);
        let cid = self.submit_admin_cmd(create_cq)?;
        self.wait_admin_completion(cid)?;

        // 创建 SQ
        let create_sq =
            SubmissionEntry::create_io_sq(0, qid, queue_pair.sq_phys().as_u64(), depth, qid);
        let cid = self.submit_admin_cmd(create_sq)?;
        self.wait_admin_completion(cid)?;

        self.io_queues.write().push(queue_pair.clone());
        Ok(queue_pair)
    }

    /// 获取寄存器访问
    #[inline]
    pub fn regs(&self) -> &NvmeRegs {
        &self.regs
    }

    /// 获取能力
    #[inline]
    pub fn capabilities(&self) -> &ControllerCapabilities {
        &self.capabilities
    }

    /// 关闭控制器
    pub fn shutdown(&self) -> Result<()> {
        // 读取当前 CC 值
        let cc_val = self.regs.cc().read();

        // 清除 SHN 位，设置正常关闭
        let new_cc = (cc_val & !(0x3 << cc::SHN_SHIFT)) | cc::SHN_NORMAL;
        self.regs.cc().write(new_cc);

        // 等待关闭完成
        self.regs
            .wait_shutdown()
            .map_err(|_| Error::new(ETIMEDOUT))?;

        Ok(())
    }

    /// 获取 Namespace
    pub fn get_namespace(self: &Arc<Self>, nsid: u32) -> Result<Arc<NvmeNamespace>> {
        if let Some(ns) = self.namespaces.read().get(&nsid) {
            return Ok(ns.clone());
        }

        let info = self.identify_namespace(nsid)?;

        let io_queue = if let Some(q) = self.io_queues.read().first() {
            q.clone()
        } else {
            self.create_io_queue()?
        };

        let namespace = Arc::new(NvmeNamespace::new(self.clone(), info, io_queue));

        self.namespaces.write().insert(nsid, namespace.clone());
        Ok(namespace)
    }

    /// 识别 Namespace
    fn identify_namespace(&self, nsid: u32) -> Result<NamespaceInfo> {
        let buffer = DmaRegion::allocate(4096).map_err(|_| Error::new(ENOMEM))?;

        let entry = SubmissionEntry::identify(
            0,
            nsid,
            0, // CNS = 0 for Namespace
            buffer.phys_addr().as_u64(),
        );

        let cid = self.submit_admin_cmd(entry)?;
        self.wait_admin_completion(cid)?;

        // 解析 Namespace 信息
        let data = buffer.as_slice();
        let size = u64::from_le_bytes(data[0..8].try_into().unwrap());
        let ncap = u64::from_le_bytes(data[8..16].try_into().unwrap());
        let flbas = data[26];
        let lba_format_index = flbas & 0x0F;
        let lba_format_offset = 128 + (lba_format_index as usize) * 4;
        let lba_format = u32::from_le_bytes(
            data[lba_format_offset..lba_format_offset + 4]
                .try_into()
                .unwrap(),
        );
        let lbads = ((lba_format >> 16) & 0xFF) as u32;
        let block_size = 1u32 << lbads;

        Ok(NamespaceInfo {
            nsid,
            size,
            capacity: ncap * block_size as u64,
            block_size,
            formatted_lba_size: lba_format_index,
        })
    }
}

/// NVMe Namespace
pub struct NvmeNamespace {
    /// 所属控制器
    controller: Arc<NvmeController>,
    /// Namespace 信息
    info: NamespaceInfo,
    /// I/O 队列
    io_queue: Arc<QueuePair>,
}

impl NvmeNamespace {
    pub fn new(
        controller: Arc<NvmeController>,
        info: NamespaceInfo,
        io_queue: Arc<QueuePair>,
    ) -> Self {
        Self {
            controller,
            info,
            io_queue,
        }
    }

    /// 读取块
    pub fn read(&self, lba: u64, buffer: &DmaRegion, block_count: u16) -> Result<()> {
        let data_len = block_count as usize * self.info.block_size as usize;
        if buffer.size() < data_len {
            return Err(Error::new(EINVAL));
        }

        let prp = PrpBuilder::new(buffer, 0, data_len)?;
        let entry =
            SubmissionEntry::read(0, self.info.nsid, lba, block_count, prp.prp1(), prp.prp2());

        // 使用 controller.regs() 而不是直接的 mmio
        let cid = self
            .io_queue
            .submit(self.controller.regs(), entry, Some(prp), None)?;

        self.io_queue.wait_completion(self.controller.regs(), cid)?;
        Ok(())
    }

    /// 写入块
    pub fn write(&self, lba: u64, buffer: &DmaRegion, block_count: u16) -> Result<()> {
        let data_len = block_count as usize * self.info.block_size as usize;
        if buffer.size() < data_len {
            return Err(Error::new(EINVAL));
        }

        let prp = PrpBuilder::new(buffer, 0, data_len)?;
        let entry =
            SubmissionEntry::write(0, self.info.nsid, lba, block_count, prp.prp1(), prp.prp2());

        let cid = self
            .io_queue
            .submit(self.controller.regs(), entry, Some(prp), None)?;

        self.io_queue.wait_completion(self.controller.regs(), cid)?;
        Ok(())
    }

    /// 读取到用户缓冲区
    ///
    /// 内部分配 DMA 缓冲区并复制数据
    pub fn read_to_slice(&self, lba: u64, buf: &mut [u8]) -> Result<()> {
        let block_size = self.info.block_size as usize;
        let block_count = (buf.len() + block_size - 1) / block_size;

        if block_count > u16::MAX as usize {
            return Err(Error::new(EINVAL));
        }

        let dma_buffer =
            DmaRegion::allocate(block_count * block_size).map_err(|_| Error::new(ENOMEM))?;
        self.read(lba, &dma_buffer, block_count as u16)?;

        // 复制数据到用户缓冲区
        buf.copy_from_slice(&dma_buffer.as_slice()[..buf.len()]);

        Ok(())
    }

    /// 从用户缓冲区写入
    ///
    /// 内部分配 DMA 缓冲区并复制数据
    pub fn write_from_slice(&self, lba: u64, buf: &[u8]) -> Result<()> {
        let block_size = self.info.block_size as usize;
        let block_count = (buf.len() + block_size - 1) / block_size;

        if block_count > u16::MAX as usize {
            return Err(Error::new(EINVAL));
        }

        let mut dma_buffer =
            DmaRegion::allocate(block_count * block_size).map_err(|_| Error::new(ENOMEM))?;
        dma_buffer.zero();

        // 复制数据到 DMA 缓冲区
        dma_buffer.as_mut_slice()[..buf.len()].copy_from_slice(buf);

        self.write(lba, &dma_buffer, block_count as u16)?;

        Ok(())
    }

    /// 异步读取（返回命令 ID）
    pub fn read_async(&self, lba: u64, buffer: Arc<DmaRegion>, block_count: u16) -> Result<u16> {
        let data_len = block_count as usize * self.info.block_size as usize;
        if buffer.size() < data_len {
            return Err(Error::new(EINVAL));
        }

        let prp = PrpBuilder::new(&buffer, 0, data_len)?;

        let entry =
            SubmissionEntry::read(0, self.info.nsid, lba, block_count, prp.prp1(), prp.prp2());

        self.io_queue
            .submit(self.controller.regs(), entry, Some(prp), Some(buffer))
    }

    /// 异步写入（返回命令 ID）
    pub fn write_async(&self, lba: u64, buffer: Arc<DmaRegion>, block_count: u16) -> Result<u16> {
        let data_len = block_count as usize * self.info.block_size as usize;
        if buffer.size() < data_len {
            return Err(Error::new(EINVAL));
        }

        let prp = PrpBuilder::new(&buffer, 0, data_len)?;

        let entry =
            SubmissionEntry::write(0, self.info.nsid, lba, block_count, prp.prp1(), prp.prp2());

        self.io_queue
            .submit(self.controller.regs(), entry, Some(prp), Some(buffer))
    }

    /// 等待命令完成
    pub fn wait(&self, cid: u16) -> Result<()> {
        self.io_queue.wait_completion(self.controller.regs(), cid)?;
        Ok(())
    }

    /// 轮询完成
    pub fn poll(&self) -> Option<CompletionEntry> {
        self.io_queue.poll_completion(self.controller.regs())
    }

    /// Flush
    pub fn flush(&self) -> Result<()> {
        let entry = SubmissionEntry::flush(0, self.info.nsid);

        let cid = self
            .io_queue
            .submit(self.controller.regs(), entry, None, None)?;

        self.io_queue.wait_completion(self.controller.regs(), cid)?;

        Ok(())
    }

    pub fn info(&self) -> NamespaceInfo {
        self.info.clone()
    }
}
