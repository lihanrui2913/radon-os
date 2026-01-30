use libdriver::{MmioRegion, define_regs};

// NVMe 寄存器偏移常量
pub mod offsets {
    pub const CAP: usize = 0x00; // Controller Capabilities (64-bit)
    pub const VS: usize = 0x08; // Version
    pub const INTMS: usize = 0x0C; // Interrupt Mask Set
    pub const INTMC: usize = 0x10; // Interrupt Mask Clear
    pub const CC: usize = 0x14; // Controller Configuration
    pub const CSTS: usize = 0x1C; // Controller Status
    pub const NSSR: usize = 0x20; // NVM Subsystem Reset
    pub const AQA: usize = 0x24; // Admin Queue Attributes
    pub const ASQ: usize = 0x28; // Admin Submission Queue Base Address
    pub const ACQ: usize = 0x30; // Admin Completion Queue Base Address
    pub const DOORBELL_BASE: usize = 0x1000;
}

define_regs! {
    pub struct NvmeRegs {
        /// Controller Capabilities - 控制器能力
        cap: u64 where offsets::CAP,

        /// Version - 版本
        vs: u32 where offsets::VS,

        /// Interrupt Mask Set - 中断屏蔽设置
        intms: u32 where offsets::INTMS,

        /// Interrupt Mask Clear - 中断屏蔽清除
        intmc: u32 where offsets::INTMC,

        /// Controller Configuration - 控制器配置
        cc: u32 where offsets::CC,

        /// Controller Status - 控制器状态
        csts: u32 where offsets::CSTS,

        /// NVM Subsystem Reset - 子系统复位
        nssr: u32 where offsets::NSSR,

        /// Admin Queue Attributes - 管理队列属性
        aqa: u32 where offsets::AQA,

        /// Admin Submission Queue Base Address - 管理提交队列基地址
        asq: u64 where offsets::ASQ,

        /// Admin Completion Queue Base Address - 管理完成队列基地址
        acq: u64 where offsets::ACQ,
    }
}

/// CAP (Controller Capabilities) 寄存器位域
pub mod cap {
    /// Maximum Queue Entries Supported (bits 0-15)
    pub const MQES_MASK: u64 = 0xFFFF;
    /// Contiguous Queues Required (bit 16)
    pub const CQR: u64 = 1 << 16;
    /// Arbitration Mechanism Supported (bits 17-18)
    pub const AMS_MASK: u64 = 0x3 << 17;
    /// Timeout (bits 24-31) - in 500ms units
    pub const TO_SHIFT: u64 = 24;
    pub const TO_MASK: u64 = 0xFF << 24;
    /// Doorbell Stride (bits 32-35)
    pub const DSTRD_SHIFT: u64 = 32;
    pub const DSTRD_MASK: u64 = 0xF << 32;
    /// NVM Subsystem Reset Supported (bit 36)
    pub const NSSRS: u64 = 1 << 36;
    /// Command Sets Supported (bits 37-44)
    pub const CSS_SHIFT: u64 = 37;
    pub const CSS_NVM: u64 = 1 << 37;
    /// Boot Partition Support (bit 45)
    pub const BPS: u64 = 1 << 45;
    /// Memory Page Size Minimum (bits 48-51)
    pub const MPSMIN_SHIFT: u64 = 48;
    pub const MPSMIN_MASK: u64 = 0xF << 48;
    /// Memory Page Size Maximum (bits 52-55)
    pub const MPSMAX_SHIFT: u64 = 52;
    pub const MPSMAX_MASK: u64 = 0xF << 52;

    /// 提取最大队列条目数
    #[inline]
    pub fn mqes(cap: u64) -> u16 {
        ((cap & MQES_MASK) + 1) as u16
    }

    /// 提取门铃步长
    #[inline]
    pub fn dstrd(cap: u64) -> usize {
        ((cap >> DSTRD_SHIFT) & 0xF) as usize
    }

    /// 提取最小页大小 (bytes)
    #[inline]
    pub fn mpsmin(cap: u64) -> usize {
        1 << (12 + ((cap >> MPSMIN_SHIFT) & 0xF))
    }

    /// 提取最大页大小 (bytes)
    #[inline]
    pub fn mpsmax(cap: u64) -> usize {
        1 << (12 + ((cap >> MPSMAX_SHIFT) & 0xF))
    }

    /// 提取超时值 (ms)
    #[inline]
    pub fn timeout_ms(cap: u64) -> u32 {
        (((cap >> TO_SHIFT) & 0xFF) as u32) * 500
    }
}

/// CC (Controller Configuration) 寄存器位域
pub mod cc {
    /// Enable (bit 0)
    pub const EN: u32 = 1 << 0;
    /// I/O Command Set Selected (bits 4-6)
    pub const CSS_SHIFT: u32 = 4;
    pub const CSS_NVM: u32 = 0 << 4;
    /// Memory Page Size (bits 7-10)
    pub const MPS_SHIFT: u32 = 7;
    pub const MPS_MASK: u32 = 0xF << 7;
    /// Arbitration Mechanism Selected (bits 11-13)
    pub const AMS_SHIFT: u32 = 11;
    pub const AMS_RR: u32 = 0 << 11; // Round Robin
    pub const AMS_WRR: u32 = 1 << 11; // Weighted Round Robin
    /// Shutdown Notification (bits 14-15)
    pub const SHN_SHIFT: u32 = 14;
    pub const SHN_NONE: u32 = 0 << 14;
    pub const SHN_NORMAL: u32 = 1 << 14;
    pub const SHN_ABRUPT: u32 = 2 << 14;
    /// I/O Submission Queue Entry Size (bits 16-19) - 2^n bytes
    pub const IOSQES_SHIFT: u32 = 16;
    /// I/O Completion Queue Entry Size (bits 20-23) - 2^n bytes
    pub const IOCQES_SHIFT: u32 = 20;

    /// 构建 CC 寄存器值
    #[inline]
    pub fn build(enable: bool, mps: u32, iosqes: u32, iocqes: u32) -> u32 {
        let mut cc = CSS_NVM | AMS_RR | SHN_NONE;
        if enable {
            cc |= EN;
        }
        cc |= (mps & 0xF) << MPS_SHIFT;
        cc |= (iosqes & 0xF) << IOSQES_SHIFT;
        cc |= (iocqes & 0xF) << IOCQES_SHIFT;
        cc
    }
}

/// CSTS (Controller Status) 寄存器位域
pub mod csts {
    /// Ready (bit 0)
    pub const RDY: u32 = 1 << 0;
    /// Controller Fatal Status (bit 1)
    pub const CFS: u32 = 1 << 1;
    /// Shutdown Status (bits 2-3)
    pub const SHST_SHIFT: u32 = 2;
    pub const SHST_MASK: u32 = 0x3 << 2;
    pub const SHST_NORMAL: u32 = 0 << 2;
    pub const SHST_OCCURRING: u32 = 1 << 2;
    pub const SHST_COMPLETE: u32 = 2 << 2;
    /// NVM Subsystem Reset Occurred (bit 4)
    pub const NSSRO: u32 = 1 << 4;
    /// Processing Paused (bit 5)
    pub const PP: u32 = 1 << 5;

    /// 检查是否就绪
    #[inline]
    pub fn is_ready(csts: u32) -> bool {
        csts & RDY != 0
    }

    /// 检查是否有致命错误
    #[inline]
    pub fn is_fatal(csts: u32) -> bool {
        csts & CFS != 0
    }

    /// 检查关闭是否完成
    #[inline]
    pub fn shutdown_complete(csts: u32) -> bool {
        (csts & SHST_MASK) == SHST_COMPLETE
    }
}

/// AQA (Admin Queue Attributes) 寄存器位域
pub mod aqa {
    /// Admin Submission Queue Size (bits 0-11) - 0-based
    pub const ASQS_MASK: u32 = 0xFFF;
    /// Admin Completion Queue Size (bits 16-27) - 0-based
    pub const ACQS_SHIFT: u32 = 16;
    pub const ACQS_MASK: u32 = 0xFFF << 16;

    /// 构建 AQA 寄存器值
    #[inline]
    pub fn build(sq_size: u16, cq_size: u16) -> u32 {
        ((cq_size - 1) as u32) << ACQS_SHIFT | ((sq_size - 1) as u32)
    }
}

impl NvmeRegs {
    /// 获取底层 MMIO 区域的引用
    pub fn mmio(&self) -> &MmioRegion {
        &self.mmio
    }

    /// 计算 SQ 门铃偏移
    ///
    /// offset = 0x1000 + (2 * qid) * (4 << dstrd)
    #[inline]
    pub fn sq_doorbell_offset(&self, qid: u16, dstrd: usize) -> usize {
        offsets::DOORBELL_BASE + (2 * qid as usize) * (4 << dstrd)
    }

    /// 计算 CQ 门铃偏移
    ///
    /// offset = 0x1000 + (2 * qid + 1) * (4 << dstrd)
    #[inline]
    pub fn cq_doorbell_offset(&self, qid: u16, dstrd: usize) -> usize {
        offsets::DOORBELL_BASE + (2 * qid as usize + 1) * (4 << dstrd)
    }

    /// 写 SQ 门铃
    #[inline]
    pub fn write_sq_doorbell(&self, qid: u16, dstrd: usize, value: u16) {
        let offset = self.sq_doorbell_offset(qid, dstrd);
        self.mmio.write_u32(offset, value as u32);
    }

    /// 写 CQ 门铃
    #[inline]
    pub fn write_cq_doorbell(&self, qid: u16, dstrd: usize, value: u16) {
        let offset = self.cq_doorbell_offset(qid, dstrd);
        self.mmio.write_u32(offset, value as u32);
    }

    /// 读取并解析控制器能力
    pub fn read_capabilities(&self) -> ControllerCapabilities {
        let cap = self.cap().read();
        ControllerCapabilities {
            max_queue_entries: cap::mqes(cap),
            doorbell_stride: cap::dstrd(cap),
            min_page_size: cap::mpsmin(cap),
            max_page_size: cap::mpsmax(cap),
            timeout_ms: cap::timeout_ms(cap),
        }
    }

    /// 等待控制器就绪
    pub fn wait_ready(&self, _timeout_ms: u32) -> Result<(), &'static str> {
        // TODO: 实现真正的超时
        loop {
            let csts = self.csts().read();
            if csts::is_fatal(csts) {
                return Err("Controller fatal error");
            }
            if csts::is_ready(csts) {
                return Ok(());
            }
            core::hint::spin_loop();
        }
    }

    /// 等待控制器禁用
    pub fn wait_disabled(&self, _timeout_ms: u32) -> Result<(), &'static str> {
        loop {
            let csts = self.csts().read();
            if csts::is_fatal(csts) {
                return Err("Controller fatal error");
            }
            if !csts::is_ready(csts) {
                return Ok(());
            }
            core::hint::spin_loop();
        }
    }

    /// 等待关闭完成
    pub fn wait_shutdown(&self) -> Result<(), &'static str> {
        loop {
            let csts = self.csts().read();
            if csts::shutdown_complete(csts) {
                return Ok(());
            }
            core::hint::spin_loop();
        }
    }
}

/// 控制器能力（解析后）
#[derive(Debug, Clone, Copy)]
pub struct ControllerCapabilities {
    /// 最大队列条目数
    pub max_queue_entries: u16,
    /// 门铃步长
    pub doorbell_stride: usize,
    /// 最小内存页大小 (bytes)
    pub min_page_size: usize,
    /// 最大内存页大小 (bytes)
    pub max_page_size: usize,
    /// 超时 (ms)
    pub timeout_ms: u32,
}
