// Minimal ramfb (RAM framebuffer) driver for QEMU
// Uses fw_cfg MMIO to configure a simple pixel framebuffer

use alloc::vec;
use alloc::vec::Vec;
use core::ptr;
use core::sync::atomic::{fence, Ordering};

pub const FB_WIDTH: usize = 320;
pub const FB_HEIGHT: usize = 240;
const FB_BPP: usize = 4;
const FB_STRIDE: usize = FB_WIDTH * FB_BPP;
const FB_SIZE: usize = FB_STRIDE * FB_HEIGHT;

const FW_CFG_FILE_DIR: u16 = 0x0019;
const FW_CFG_DMA_WRITE: u32 = 1 << 4;
const FW_CFG_DMA_SELECT: u32 = 1 << 3;
const DRM_FORMAT_XRGB8888: u32 = 0x34325258;

#[repr(C, packed)]
struct RamfbCfg {
    addr: u64,
    fourcc: u32,
    flags: u32,
    width: u32,
    height: u32,
    stride: u32,
}

pub struct Ramfb {
    fb_buf: Vec<u8>,
}

#[allow(dead_code)]

impl Ramfb {
    pub fn init() -> crate::Result<Self> {
        use alloc::string::ToString;
        use core::ops::DerefMut;
        use kmem_core::{AddressRangeExt, PhysicalAddress};

        let phys_offset = crate::state::global().boot_info.physical_address_offset.get();

        // Map fw_cfg MMIO (phys 0x10100000)
        let page_size = crate::arch::PAGE_SIZE;
        let fw_mmap = crate::mem::with_kernel_aspace(|aspace| {
            let phys_range = core::ops::Range::from_start_len(
                PhysicalAddress::new(0x10100000usize), page_size,
            );
            let mmap = crate::mem::Mmap::new_phys(
                aspace.clone(), phys_range, page_size, page_size,
                Some("fw_cfg".to_string()),
            )?;
            mmap.commit(aspace.lock().deref_mut(), 0..page_size, true)?;
            Ok::<_, anyhow::Error>(mmap)
        })?;
        let fw_cfg_virt = fw_mmap.range().start.get();
        core::mem::forget(fw_mmap);
        tracing::info!("fw_cfg mapped at virt {:#x}", fw_cfg_virt);

        // Find etc/ramfb selector
        let ramfb_sel = find_fw_cfg_file(fw_cfg_virt, "etc/ramfb")?;
        tracing::info!("etc/ramfb selector: {:#x}", ramfb_sel);

        // Allocate framebuffer (heap-allocated, so it has a virtual address)
        // We need the physical address for ramfb config
        let mut fb_buf = vec![0u8; FB_SIZE];
        let fb_virt_addr = fb_buf.as_ptr() as usize;

        let fb_phys = fb_virt_addr - phys_offset;
        tracing::info!("FB virt={:#x} phys={:#x}", fb_virt_addr, fb_phys);

        // Build ramfb config + DMA descriptor in heap memory
        // (stack memory maps to addresses outside RAM range)
        // Layout: [cfg 28 bytes] [pad 4] [dma_control 4] [dma_length 4] [dma_address 8] = 48 bytes
        let mut dma_buf = vec![0u8; 48];

        // cfg: addr (8 bytes, little-endian)
        dma_buf[0..8].copy_from_slice(&(fb_phys as u64).to_be_bytes());
        // cfg: fourcc (4 bytes, big-endian)
        dma_buf[8..12].copy_from_slice(&DRM_FORMAT_XRGB8888.to_be_bytes());
        // cfg: flags = 0 (already zero)
        // cfg: width (4 bytes, big-endian)
        dma_buf[16..20].copy_from_slice(&(FB_WIDTH as u32).to_be_bytes());
        // cfg: height (4 bytes, big-endian)
        dma_buf[20..24].copy_from_slice(&(FB_HEIGHT as u32).to_be_bytes());
        // cfg: stride (4 bytes, big-endian)
        dma_buf[24..28].copy_from_slice(&(FB_STRIDE as u32).to_be_bytes());

        // DMA descriptor at offset 32
        let cfg_virt = dma_buf.as_ptr() as usize;
        let cfg_phys = cfg_virt - phys_offset;
        let dma_desc_offset = 32;

        // dma_control
        let control = (FW_CFG_DMA_WRITE | FW_CFG_DMA_SELECT | ((ramfb_sel as u32) << 16)).to_be();
        dma_buf[dma_desc_offset..dma_desc_offset+4].copy_from_slice(&control.to_ne_bytes());
        // dma_length
        dma_buf[dma_desc_offset+4..dma_desc_offset+8].copy_from_slice(&28u32.to_be().to_ne_bytes());
        // dma_address (physical address of cfg data)
        dma_buf[dma_desc_offset+8..dma_desc_offset+16].copy_from_slice(&(cfg_phys as u64).to_be().to_ne_bytes());

        let dma_desc_virt = cfg_virt + dma_desc_offset;
        let dma_desc_phys = dma_desc_virt - phys_offset;

        tracing::info!("DMA: cfg_phys={:#x} desc_phys={:#x}", cfg_phys, dma_desc_phys);

        fence(Ordering::SeqCst);
        unsafe {
            ptr::write_volatile(
                (fw_cfg_virt + 0x10) as *mut u64,
                (dma_desc_phys as u64).to_be(),
            );
        }
        fence(Ordering::SeqCst);

        // Wait for DMA completion (control field at dma_buf[32..36] becomes 0)
        for _ in 0..10_000_000 {
            fence(Ordering::Acquire);
            let ctrl = unsafe { ptr::read_volatile(dma_buf.as_ptr().add(dma_desc_offset) as *const u32) };
            if ctrl == 0 {
                tracing::info!("ramfb DMA completed");
                break;
            }
            core::hint::spin_loop();
        }
        let final_ctrl = unsafe { ptr::read_volatile(dma_buf.as_ptr().add(dma_desc_offset) as *const u32) };
        if final_ctrl != 0 {
            tracing::warn!("ramfb DMA timed out, control={:#x}", final_ctrl);
        }
        tracing::info!("ramfb configured: {}x{}", FB_WIDTH, FB_HEIGHT);

        Ok(Self { fb_buf })
    }

    /// Blit an indexed framebuffer (320x240 u8) to XRGB8888 using the given palette.
    pub fn blit(&mut self, indexed_fb: &[u8], palette: &[(u8, u8, u8); 256]) {
        let fb = &mut self.fb_buf;
        for i in 0..(FB_WIDTH * FB_HEIGHT) {
            let idx = indexed_fb[i] as usize;
            let (r, g, b) = palette[idx];
            let off = i * FB_BPP;
            fb[off] = b;
            fb[off + 1] = g;
            fb[off + 2] = r;
            fb[off + 3] = 0xFF;
        }
        fence(Ordering::Release);
    }
}

fn find_fw_cfg_file(base: usize, name: &str) -> crate::Result<u16> {
    // First verify fw_cfg by reading signature (selector 0x0000)
    unsafe {
        ptr::write_volatile((base + 0x08) as *mut u16, 0x0000u16);
    }
    fence(Ordering::SeqCst);
    let mut sig = [0u8; 4];
    for b in &mut sig {
        *b = unsafe { ptr::read_volatile(base as *const u8) };
    }
    let sig_str = core::str::from_utf8(&sig).unwrap_or("????");
    tracing::debug!("fw_cfg signature: {:?} ({:#x} {:#x} {:#x} {:#x})", sig_str, sig[0], sig[1], sig[2], sig[3]);

    // Select file directory (selector 0x0019)
    // On MMIO, selector is big-endian 16-bit
    unsafe {
        ptr::write_volatile((base + 0x08) as *mut u16, FW_CFG_FILE_DIR.to_be());
    }
    fence(Ordering::SeqCst);

    // Read file count (big-endian u32, read byte by byte)
    let mut count_bytes = [0u8; 4];
    for b in &mut count_bytes {
        *b = unsafe { ptr::read_volatile(base as *const u8) };
    }
    let count = u32::from_be_bytes(count_bytes) as usize;
    tracing::debug!("fw_cfg: {count} files in directory (raw: {:#x} {:#x} {:#x} {:#x})",
        count_bytes[0], count_bytes[1], count_bytes[2], count_bytes[3]);

    for i in 0..count {
        let mut entry = [0u8; 64];
        for b in &mut entry {
            *b = unsafe { ptr::read_volatile(base as *const u8) };
        }
        let select = u16::from_be_bytes([entry[4], entry[5]]);
        let file_name = core::str::from_utf8(&entry[8..])
            .unwrap_or("")
            .trim_end_matches('\0');

        if i < 5 || file_name.contains("ramfb") {
            tracing::debug!("  fw_cfg file[{i}]: sel={select:#x} name={file_name:?}");
        }

        if file_name == name {
            return Ok(select);
        }
    }
    anyhow::bail!("fw_cfg file {name} not found (searched {count} entries)");
}

fn fw_cfg_dma_write(base: usize, selector: u16, data: &RamfbCfg, phys_offset: usize) {
    #[repr(C, align(16))]
    struct DmaDesc {
        control: u32,
        length: u32,
        address: u64,
    }

    let data_virt = data as *const RamfbCfg as usize;
    let data_phys = data_virt - phys_offset;
    let data_len = core::mem::size_of::<RamfbCfg>();

    let desc = DmaDesc {
        control: (FW_CFG_DMA_WRITE | FW_CFG_DMA_SELECT | ((selector as u32) << 16)).to_be(),
        length: (data_len as u32).to_be(),
        address: (data_phys as u64).to_be(),
    };

    let desc_virt = &desc as *const DmaDesc as usize;
    let desc_phys = desc_virt - phys_offset;

    fence(Ordering::SeqCst);
    unsafe {
        ptr::write_volatile((base + 0x10) as *mut u64, (desc_phys as u64).to_be());
    }
    fence(Ordering::SeqCst);

    // Wait for completion
    for _ in 0..1_000_000 {
        fence(Ordering::SeqCst);
        let ctrl = unsafe { ptr::read_volatile(&desc.control) };
        if ctrl == 0 {
            return;
        }
        core::hint::spin_loop();
    }
    tracing::warn!("fw_cfg DMA timed out");
}
