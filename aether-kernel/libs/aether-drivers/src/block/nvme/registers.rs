use aether_frame::io::{Mmio, MmioRegion, RemapError, remap_mmio};

const NVME_REG_CAP: usize = 0x00;
const NVME_REG_VS: usize = 0x08;
const NVME_REG_CC: usize = 0x14;
const NVME_REG_CSTS: usize = 0x1c;
const NVME_REG_AQA: usize = 0x24;
const NVME_REG_ASQ: usize = 0x28;
const NVME_REG_ACQ: usize = 0x30;
const NVME_DOORBELL_BASE: usize = 0x1000;

#[derive(Debug, Clone, Copy)]
pub struct NvmeVersion {
    pub major: u16,
    pub minor: u8,
    pub tertiary: u8,
}

#[derive(Debug, Clone, Copy)]
pub struct NvmeCapabilities {
    pub max_queue_entries: u16,
    pub doorbell_stride: u8,
    pub timeout_units_500ms: u8,
    pub min_page_shift: u8,
    pub max_page_shift: u8,
}

impl NvmeCapabilities {
    pub fn from_raw(raw: u64) -> Self {
        Self {
            max_queue_entries: ((raw & 0xffff) as u16).saturating_add(1),
            timeout_units_500ms: ((raw >> 24) & 0xff) as u8,
            doorbell_stride: ((raw >> 32) & 0x0f) as u8,
            min_page_shift: (((raw >> 48) & 0x0f) as u8).saturating_add(12),
            max_page_shift: (((raw >> 52) & 0x0f) as u8).saturating_add(12),
        }
    }

    pub const fn doorbell_step(self) -> usize {
        4usize << self.doorbell_stride
    }
}

#[derive(Clone, Copy)]
pub struct NvmeRegisters {
    region: MmioRegion,
}

impl NvmeRegisters {
    pub fn map(base: u64) -> Result<Self, RemapError> {
        Ok(Self {
            region: remap_mmio(base, 0x2000)?,
        })
    }

    pub fn capabilities(&self) -> NvmeCapabilities {
        NvmeCapabilities::from_raw(self.read64(NVME_REG_CAP))
    }

    pub fn version(&self) -> NvmeVersion {
        let raw = self.read32(NVME_REG_VS);
        NvmeVersion {
            major: ((raw >> 16) & 0xffff) as u16,
            minor: ((raw >> 8) & 0xff) as u8,
            tertiary: (raw & 0xff) as u8,
        }
    }

    pub fn controller_ready(&self) -> bool {
        (self.read32(NVME_REG_CSTS) & 0x1) != 0
    }

    pub fn set_admin_queues(&self, depth: u16, submission_phys: u64, completion_phys: u64) {
        let qsize = depth.saturating_sub(1) as u32;
        self.write32(NVME_REG_AQA, qsize | (qsize << 16));
        self.write64(NVME_REG_ASQ, submission_phys);
        self.write64(NVME_REG_ACQ, completion_phys);
    }

    pub fn disable_controller(&self) {
        let mut cc = self.read32(NVME_REG_CC);
        cc &= !0x1;
        self.write32(NVME_REG_CC, cc);
    }

    pub fn enable_controller(&self, memory_page_shift: u8) {
        let page_size = memory_page_shift.saturating_sub(12) as u32;
        let cc = 0x1 | (page_size << 7) | (6 << 16) | (4 << 20);
        self.write32(NVME_REG_CC, cc);
    }

    pub fn ring_submission_doorbell(&self, queue_id: u16, tail: u16, caps: NvmeCapabilities) {
        self.write32(self.submission_doorbell_offset(queue_id, caps), tail as u32);
    }

    pub fn ring_completion_doorbell(&self, queue_id: u16, head: u16, caps: NvmeCapabilities) {
        self.write32(self.completion_doorbell_offset(queue_id, caps), head as u32);
    }

    fn submission_doorbell_offset(&self, queue_id: u16, caps: NvmeCapabilities) -> usize {
        NVME_DOORBELL_BASE + (queue_id as usize * 2) * caps.doorbell_step()
    }

    fn completion_doorbell_offset(&self, queue_id: u16, caps: NvmeCapabilities) -> usize {
        self.submission_doorbell_offset(queue_id, caps) + caps.doorbell_step()
    }

    fn read32(&self, offset: usize) -> u32 {
        let register: Mmio<u32> = unsafe {
            self.region
                .register(offset)
                .expect("nvme register offset must be in range")
        };
        register.read()
    }

    fn write32(&self, offset: usize, value: u32) {
        let register: Mmio<u32> = unsafe {
            self.region
                .register(offset)
                .expect("nvme register offset must be in range")
        };
        register.write(value);
    }

    fn read64(&self, offset: usize) -> u64 {
        let low = self.read32(offset) as u64;
        let high = self.read32(offset + 4) as u64;
        (high << 32) | low
    }

    fn write64(&self, offset: usize, value: u64) {
        self.write32(offset, value as u32);
        self.write32(offset + 4, (value >> 32) as u32);
    }
}
