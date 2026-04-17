use core::ffi::{c_char, c_void};
use core::mem::transmute;
use core::ptr;
use core::slice;
use core::str;
use core::sync::atomic::{AtomicUsize, Ordering, compiler_fence};

use super::{
    BootInfo, BootProtocol, BootProtocolParser, Cpu, CpuTopology, FramebufferInfo, MemoryMap,
    MemoryRegion, MemoryRegionKind, PixelBitfield, PixelLayout,
};

pub const MAX_MEMORY_REGIONS: usize = 256;
pub const MAX_CPUS: usize = crate::libs::percpu::MAX_PERCPU_CPUS;

const LIMINE_COMMON_MAGIC_0: u64 = 0xc7b1_dd30_df4c_8b88;
const LIMINE_COMMON_MAGIC_1: u64 = 0x0a82_e883_a194_f07b;

const LIMINE_BASE_REVISION_MAGIC_0: u64 = 0xf956_2b2d_5c95_a6c8;
const LIMINE_BASE_REVISION_MAGIC_1: u64 = 0x6a7b_3849_4453_6bdc;
const LIMINE_BASE_REVISION: u64 = 6;

const LIMINE_REQUESTS_START_MARKER_0: u64 = 0xf6b8_f4b3_9de7_d1ae;
const LIMINE_REQUESTS_START_MARKER_1: u64 = 0xfab9_1a69_40fc_b9cf;
const LIMINE_REQUESTS_END_MARKER_0: u64 = 0xadc0_e053_1bb1_0d03;
const LIMINE_REQUESTS_END_MARKER_1: u64 = 0x9572_709f_3176_4c62;

const LIMINE_FRAMEBUFFER_REQUEST_ID_0: u64 = 0x9d58_27dc_d881_dd75;
const LIMINE_FRAMEBUFFER_REQUEST_ID_1: u64 = 0xa314_8604_f6fa_b11b;
const LIMINE_HHDM_REQUEST_ID_0: u64 = 0x48dc_f1cb_8ad2_b852;
const LIMINE_HHDM_REQUEST_ID_1: u64 = 0x6398_4e95_9a98_244b;
const LIMINE_MEMMAP_REQUEST_ID_0: u64 = 0x67cf_3d9d_378a_806f;
const LIMINE_MEMMAP_REQUEST_ID_1: u64 = 0xe304_acdf_c50c_3c62;
const LIMINE_RSDP_REQUEST_ID_0: u64 = 0xc5e7_7b6b_397e_7b43;
const LIMINE_RSDP_REQUEST_ID_1: u64 = 0x2763_7845_accd_cf3c;
const LIMINE_KERNEL_FILE_REQUEST_ID_0: u64 = 0xad97_e90e_83f1_ed67;
const LIMINE_KERNEL_FILE_REQUEST_ID_1: u64 = 0x31eb_5d1c_5ff2_3b69;
const LIMINE_MODULE_REQUEST_ID_0: u64 = 0x3e7e_2797_02be_32af;
const LIMINE_MODULE_REQUEST_ID_1: u64 = 0xca1c_4f3b_d128_0cee;
const LIMINE_MP_REQUEST_ID_0: u64 = 0x95a6_7b81_9a1b_857e;
const LIMINE_MP_REQUEST_ID_1: u64 = 0xa0b6_1b72_3b6a_73e0;
const LIMINE_BOOT_TIME_REQUEST_ID_0: u64 = 0x5027_46e1_84c0_88aa;
const LIMINE_BOOT_TIME_REQUEST_ID_1: u64 = 0xfbc5_ec83_e632_7893;

const LIMINE_FRAMEBUFFER_MEMORY_MODEL_RGB: u8 = 1;

#[repr(C)]
struct Request<T> {
    common_magic_0: u64,
    common_magic_1: u64,
    request_id_0: u64,
    request_id_1: u64,
    revision: u64,
    response: *const T,
}

impl<T> Request<T> {
    const fn new(request_id_0: u64, request_id_1: u64) -> Self {
        Self {
            common_magic_0: LIMINE_COMMON_MAGIC_0,
            common_magic_1: LIMINE_COMMON_MAGIC_1,
            request_id_0,
            request_id_1,
            revision: 0,
            response: ptr::null(),
        }
    }
}

#[repr(C)]
struct FramebufferRequestResponse {
    revision: u64,
    framebuffer_count: u64,
    framebuffers: *const *const LimineFramebuffer,
}

#[repr(C)]
struct HhdmRequestResponse {
    revision: u64,
    offset: u64,
}

#[repr(C)]
struct MemmapRequestResponse {
    revision: u64,
    entry_count: u64,
    entries: *const *const LimineMemmapEntry,
}

#[repr(C)]
struct RsdpRequestResponse {
    revision: u64,
    address: *const c_void,
}

#[repr(C)]
struct KernelFileRequestResponse {
    revision: u64,
    kernel_file: *const LimineFile,
}

#[repr(C)]
struct ModuleRequestResponse {
    revision: u64,
    module_count: u64,
    modules: *const *const LimineFile,
}

#[repr(C)]
struct MpRequestResponse {
    revision: u64,
    flags: u32,
    bsp_lapic_id: u32,
    cpu_count: u64,
    cpus: *const *mut LimineMpInfo,
}

#[repr(C)]
struct BootTimeRequestResponse {
    revision: u64,
    boot_time: i64,
}

#[repr(C)]
struct LimineFile {
    revision: u64,
    address: *const c_void,
    size: u64,
    path: *const c_char,
    cmdline: *const c_char,
    media_type: u32,
    unused: u32,
    tftp_ip: u32,
    tftp_port: u32,
    partition_index: u32,
    mbr_disk_id: u32,
    gpt_disk_uuid: [u64; 2],
    gpt_part_uuid: [u64; 2],
    part_uuid: [u64; 2],
}

#[repr(C)]
struct LimineFramebuffer {
    address: *mut c_void,
    width: u64,
    height: u64,
    pitch: u64,
    bpp: u16,
    memory_model: u8,
    red_mask_size: u8,
    red_mask_shift: u8,
    green_mask_size: u8,
    green_mask_shift: u8,
    blue_mask_size: u8,
    blue_mask_shift: u8,
    unused: [u8; 7],
    edid_size: u64,
    edid: *const c_void,
}

#[repr(C)]
struct LimineMemmapEntry {
    base: u64,
    length: u64,
    kind: u64,
}

#[repr(C)]
struct LimineMpInfo {
    processor_id: u32,
    lapic_id: u32,
    reserved: u64,
    goto_address: Option<unsafe extern "C" fn(*mut Self) -> !>,
    extra_argument: u64,
}

#[used]
#[unsafe(link_section = ".limine_requests_start")]
static LIMINE_REQUESTS_START_MARKER: [u64; 2] = [
    LIMINE_REQUESTS_START_MARKER_0,
    LIMINE_REQUESTS_START_MARKER_1,
];

#[used]
#[unsafe(link_section = ".limine_requests")]
static mut LIMINE_BASE_REVISION_TAG: [u64; 3] = [
    LIMINE_BASE_REVISION_MAGIC_0,
    LIMINE_BASE_REVISION_MAGIC_1,
    LIMINE_BASE_REVISION,
];

#[used]
#[unsafe(link_section = ".limine_requests")]
static mut FRAMEBUFFER_REQUEST: Request<FramebufferRequestResponse> = Request::new(
    LIMINE_FRAMEBUFFER_REQUEST_ID_0,
    LIMINE_FRAMEBUFFER_REQUEST_ID_1,
);

#[used]
#[unsafe(link_section = ".limine_requests")]
static mut HHDM_REQUEST: Request<HhdmRequestResponse> =
    Request::new(LIMINE_HHDM_REQUEST_ID_0, LIMINE_HHDM_REQUEST_ID_1);

#[used]
#[unsafe(link_section = ".limine_requests")]
static mut MEMMAP_REQUEST: Request<MemmapRequestResponse> =
    Request::new(LIMINE_MEMMAP_REQUEST_ID_0, LIMINE_MEMMAP_REQUEST_ID_1);

#[used]
#[unsafe(link_section = ".limine_requests")]
static mut RSDP_REQUEST: Request<RsdpRequestResponse> =
    Request::new(LIMINE_RSDP_REQUEST_ID_0, LIMINE_RSDP_REQUEST_ID_1);

#[used]
#[unsafe(link_section = ".limine_requests")]
static mut KERNEL_FILE_REQUEST: Request<KernelFileRequestResponse> = Request::new(
    LIMINE_KERNEL_FILE_REQUEST_ID_0,
    LIMINE_KERNEL_FILE_REQUEST_ID_1,
);

#[used]
#[unsafe(link_section = ".limine_requests")]
static mut MODULE_REQUEST: Request<ModuleRequestResponse> =
    Request::new(LIMINE_MODULE_REQUEST_ID_0, LIMINE_MODULE_REQUEST_ID_1);

#[used]
#[unsafe(link_section = ".limine_requests")]
static mut MP_REQUEST: Request<MpRequestResponse> =
    Request::new(LIMINE_MP_REQUEST_ID_0, LIMINE_MP_REQUEST_ID_1);

#[used]
#[unsafe(link_section = ".limine_requests")]
static mut BOOT_TIME_REQUEST: Request<BootTimeRequestResponse> =
    Request::new(LIMINE_BOOT_TIME_REQUEST_ID_0, LIMINE_BOOT_TIME_REQUEST_ID_1);

#[used]
#[unsafe(link_section = ".limine_requests_end")]
static LIMINE_REQUESTS_END_MARKER: [u64; 2] =
    [LIMINE_REQUESTS_END_MARKER_0, LIMINE_REQUESTS_END_MARKER_1];

pub struct LimineProtocol;

impl BootProtocolParser for LimineProtocol {
    unsafe fn parse(
        regions: &'static mut [MemoryRegion; MAX_MEMORY_REGIONS],
        cpus: &'static mut [Cpu; MAX_CPUS],
    ) -> Option<BootInfo<'static>> {
        compiler_fence(Ordering::SeqCst);

        let hhdm = unsafe { response_ref(&raw const HHDM_REQUEST) }?;
        let memmap = unsafe { response_ref(&raw const MEMMAP_REQUEST) }?;

        let framebuffer = unsafe { response_ref(&raw const FRAMEBUFFER_REQUEST) }
            .and_then(|response| unsafe { parse_framebuffer_response(response) });

        let command_line = unsafe { response_ref(&raw const KERNEL_FILE_REQUEST) }
            .and_then(|response| unsafe { parse_kernel_command_line(response) });
        let initrd = unsafe { response_ref(&raw const MODULE_REQUEST) }
            .and_then(|response| unsafe { parse_initrd(response) });

        let rsdp_addr = unsafe { response_ref(&raw const RSDP_REQUEST) }
            .map(|response| response.address as u64);
        let cpu_count = unsafe { response_ref(&raw const MP_REQUEST) }
            .map_or(0, |response| unsafe { parse_mp_response(response, cpus) });

        let boot_time = unsafe { response_ref(&raw const BOOT_TIME_REQUEST) }
            .map(|response| response.boot_time);

        let region_count = unsafe { parse_memmap_response(memmap, regions) };

        Some(BootInfo {
            boot_time,
            protocol: BootProtocol::Limine,
            command_line,
            initrd,
            rsdp_addr,
            framebuffer,
            cpus: CpuTopology::new(&cpus[..cpu_count]),
            memory_map: MemoryMap::new(&regions[..region_count]),
            hhdm_offset: hhdm.offset,
        })
    }
}

pub fn base_revision_supported() -> bool {
    compiler_fence(Ordering::SeqCst);
    unsafe { LIMINE_BASE_REVISION_TAG[2] == 0 }
}

static AP_ENTRY: AtomicUsize = AtomicUsize::new(0);
static mut SMP_CPU_ENTRIES: [*mut LimineMpInfo; MAX_CPUS] = [ptr::null_mut(); MAX_CPUS];
static SMP_CPU_COUNT: AtomicUsize = AtomicUsize::new(0);

pub unsafe fn start_secondary_cpus(entry: super::SecondaryCpuEntry) -> Result<usize, &'static str> {
    let response = unsafe { response_ref(&raw const MP_REQUEST) }
        .ok_or("limine mp response is unavailable")?;
    let cpu_count = SMP_CPU_COUNT
        .load(Ordering::Acquire)
        .min(response.cpu_count as usize);
    if cpu_count == 0 {
        return Ok(0);
    }

    AP_ENTRY.store(entry as usize, Ordering::Release);

    let mut started = 0;
    let entries = (&raw mut SMP_CPU_ENTRIES).cast::<*mut LimineMpInfo>();
    for index in 0..cpu_count {
        let cpu = unsafe { (*entries.add(index)).as_mut() }.ok_or("limine mp cpu entry is null")?;
        if cpu.lapic_id == response.bsp_lapic_id {
            continue;
        }

        cpu.extra_argument = index as u64;
        cpu.goto_address = Some(limine_mp_entry);
        started += 1;
    }

    Ok(started)
}

unsafe extern "C" fn limine_mp_entry(info: *mut LimineMpInfo) -> ! {
    let callback: super::SecondaryCpuEntry = unsafe { transmute(AP_ENTRY.load(Ordering::Acquire)) };
    let index = unsafe { (*info).extra_argument as usize };
    callback(index)
}

unsafe fn response_ref<T>(request: *const Request<T>) -> Option<&'static T> {
    let response = unsafe { (*request).response };
    (!response.is_null()).then(|| unsafe { &*response })
}

unsafe fn parse_framebuffer_response(
    response: &FramebufferRequestResponse,
) -> Option<FramebufferInfo> {
    if response.framebuffer_count == 0 || response.framebuffers.is_null() {
        return None;
    }

    let framebuffer = unsafe { *response.framebuffers };
    if framebuffer.is_null() {
        return None;
    }

    let framebuffer = unsafe { &*framebuffer };
    if framebuffer.memory_model != LIMINE_FRAMEBUFFER_MEMORY_MODEL_RGB {
        return None;
    }

    Some(FramebufferInfo {
        base: framebuffer.address as u64,
        size: framebuffer.pitch.saturating_mul(framebuffer.height),
        width: framebuffer.width as u32,
        height: framebuffer.height as u32,
        stride: framebuffer.pitch as u32,
        bits_per_pixel: framebuffer.bpp as u8,
        pixel_layout: PixelLayout {
            red: PixelBitfield {
                size: framebuffer.red_mask_size,
                shift: framebuffer.red_mask_shift,
            },
            green: PixelBitfield {
                size: framebuffer.green_mask_size,
                shift: framebuffer.green_mask_shift,
            },
            blue: PixelBitfield {
                size: framebuffer.blue_mask_size,
                shift: framebuffer.blue_mask_shift,
            },
            reserved: PixelBitfield { size: 0, shift: 0 },
        },
    })
}

unsafe fn parse_kernel_command_line(response: &KernelFileRequestResponse) -> Option<&'static str> {
    let file = unsafe { response.kernel_file.as_ref() }?;
    if file.cmdline.is_null() {
        return None;
    }

    unsafe { c_string(file.cmdline.cast::<u8>()) }
}

unsafe fn parse_initrd(response: &ModuleRequestResponse) -> Option<super::Initrd> {
    if response.module_count == 0 || response.modules.is_null() {
        return None;
    }

    let modules =
        unsafe { slice::from_raw_parts(response.modules, response.module_count as usize) };
    let module = modules
        .iter()
        .filter_map(|entry| unsafe { entry.as_ref() })
        .find(|file| {
            unsafe { limine_file_path(file) }.is_none_or(|path| {
                path.ends_with("initramfs.img") || path.ends_with("/initramfs.img")
            })
        })?;

    Some(super::Initrd {
        start: module.address as u64,
        size: module.size,
    })
}

unsafe fn parse_memmap_response(
    response: &MemmapRequestResponse,
    regions_out: &mut [MemoryRegion; MAX_MEMORY_REGIONS],
) -> usize {
    if response.entries.is_null() {
        return 0;
    }

    let count = usize::min(response.entry_count as usize, MAX_MEMORY_REGIONS);
    let entries = unsafe { slice::from_raw_parts(response.entries, count) };

    for (index, entry_ptr) in entries.iter().enumerate() {
        if entry_ptr.is_null() {
            regions_out[index] = MemoryRegion::EMPTY;
            continue;
        }

        let entry = unsafe { &**entry_ptr };
        crate::serial_println!(
            "memory region {}: [{:#x}, {:#x}]",
            index,
            entry.base,
            entry.base + entry.length
        );
        regions_out[index] = MemoryRegion {
            start: entry.base,
            len: entry.length,
            kind: MemoryRegionKind(entry.kind),
        };
    }

    count
}

unsafe fn parse_mp_response(response: &MpRequestResponse, cpus_out: &mut [Cpu; MAX_CPUS]) -> usize {
    if response.cpu_count == 0 || response.cpus.is_null() {
        SMP_CPU_COUNT.store(0, Ordering::Release);
        return 0;
    }

    let count = usize::min(response.cpu_count as usize, MAX_CPUS);
    let cpus = unsafe { slice::from_raw_parts(response.cpus, count) };

    for (index, cpu_ptr) in cpus.iter().enumerate() {
        if cpu_ptr.is_null() {
            cpus_out[index] = Cpu::EMPTY;
            continue;
        }

        let cpu = unsafe { &**cpu_ptr };
        unsafe {
            *(&raw mut SMP_CPU_ENTRIES)
                .cast::<*mut LimineMpInfo>()
                .add(index) = *cpu_ptr
        };
        cpus_out[index] = Cpu {
            processor_id: cpu.processor_id,
            lapic_id: cpu.lapic_id,
            is_bsp: cpu.lapic_id == response.bsp_lapic_id,
        };
    }

    SMP_CPU_COUNT.store(count, Ordering::Release);
    count
}

unsafe fn c_string(ptr: *const u8) -> Option<&'static str> {
    let mut len = 0usize;
    while unsafe { ptr.add(len).read() } != 0 {
        len += 1;
    }

    str::from_utf8(unsafe { slice::from_raw_parts(ptr, len) }).ok()
}

unsafe fn limine_file_path(file: &LimineFile) -> Option<&'static str> {
    if file.path.is_null() {
        return None;
    }

    unsafe { c_string(file.path.cast::<u8>()) }
}
