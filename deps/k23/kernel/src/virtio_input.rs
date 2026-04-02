// Minimal VirtIO keyboard input driver for QEMU
// Polls for key events from virtio-keyboard-device via VirtIO MMIO transport

use alloc::vec;
use core::ptr;
use core::sync::atomic::{fence, Ordering};

use fallible_iterator::FallibleIterator;

// VirtIO MMIO register offsets
const VIRTIO_MMIO_MAGIC: usize = 0x000;
const VIRTIO_MMIO_VERSION: usize = 0x004;
const VIRTIO_MMIO_DEVICE_ID: usize = 0x008;
const VIRTIO_MMIO_STATUS: usize = 0x070;
const VIRTIO_MMIO_QUEUE_SEL: usize = 0x030;
const VIRTIO_MMIO_QUEUE_NUM_MAX: usize = 0x034;
const VIRTIO_MMIO_QUEUE_NUM: usize = 0x038;
const VIRTIO_MMIO_QUEUE_READY: usize = 0x044;
const VIRTIO_MMIO_QUEUE_NOTIFY: usize = 0x050;
const VIRTIO_MMIO_INTERRUPT_ACK: usize = 0x064;
// Legacy (v1) registers
const VIRTIO_MMIO_QUEUE_PFN: usize = 0x040;
const VIRTIO_MMIO_QUEUE_ALIGN: usize = 0x03C;
const VIRTIO_MMIO_GUEST_PAGE_SIZE: usize = 0x028;
// Modern (v2) registers
const VIRTIO_MMIO_QUEUE_DESC_LOW: usize = 0x080;
const VIRTIO_MMIO_QUEUE_DESC_HIGH: usize = 0x084;
const VIRTIO_MMIO_QUEUE_DRIVER_LOW: usize = 0x090;
const VIRTIO_MMIO_QUEUE_DRIVER_HIGH: usize = 0x094;
const VIRTIO_MMIO_QUEUE_DEVICE_LOW: usize = 0x0A0;
const VIRTIO_MMIO_QUEUE_DEVICE_HIGH: usize = 0x0A4;

const VIRTIO_MAGIC: u32 = 0x74726976;
const VIRTIO_DEV_INPUT: u32 = 18;

// Status bits
const VIRTIO_STATUS_ACK: u32 = 1;
const VIRTIO_STATUS_DRIVER: u32 = 2;
const VIRTIO_STATUS_FEATURES_OK: u32 = 8;
const VIRTIO_STATUS_DRIVER_OK: u32 = 4;

const QUEUE_SIZE: u16 = 16;
const EVENT_SIZE: usize = 8; // sizeof(VirtioInputEvent)

// Linux evdev keycodes
const KEY_Q: u16 = 16;
const KEY_W: u16 = 17;
const KEY_A: u16 = 30;
const KEY_S: u16 = 31;
const KEY_D: u16 = 32;
const KEY_Z: u16 = 44;
const KEY_SPACE: u16 = 57;
const KEY_ENTER: u16 = 28;
const KEY_UP: u16 = 103;
const KEY_DOWN: u16 = 108;
const KEY_LEFT: u16 = 105;
const KEY_RIGHT: u16 = 106;

const EV_KEY: u16 = 1;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct VirtqDesc {
    addr: u64,
    len: u32,
    flags: u16,
    next: u16,
}

// Virtqueue layout allocated as a single contiguous buffer:
// [descs: QUEUE_SIZE * 16] [avail: 6 + 2*QUEUE_SIZE] [padding] [used: 6 + 8*QUEUE_SIZE]
// Plus separate event buffers: QUEUE_SIZE * 8 bytes

pub struct VirtioInput {
    base: usize,
    // Pointers into the queue allocation
    descs: *mut VirtqDesc,
    avail_flags: *mut u16,
    avail_idx: *mut u16,
    avail_ring: *mut u16,
    used_flags: *const u16,
    used_idx: *const u16,
    used_ring: *const [u32; 2], // [id, len] pairs
    // Event buffer base
    events_base: *mut u8,
    last_seen_used: u16,
    buttons: u16,
    quit: bool,
    // Keep allocations alive
    _queue_buf: alloc::vec::Vec<u8>,
    _event_buf: alloc::vec::Vec<u8>,
}

unsafe impl Send for VirtioInput {}

impl VirtioInput {
    pub fn init() -> crate::Result<Self> {
        use alloc::string::ToString;
        use core::ops::DerefMut;
        use kmem_core::{AddressRangeExt, PhysicalAddress};

        let phys_offset = crate::state::global().boot_info.physical_address_offset.get();
        let page_size = crate::arch::PAGE_SIZE;
        let devtree = &crate::state::global().device_tree;

        // Probe virtio MMIO devices at known addresses (0x10001000..0x10008000)
        let mut input_phys = 0usize;
        for addr in (0x10001000..=0x10008000).step_by(0x1000) {
            // Temporarily map to check device ID
            let mmap = crate::mem::with_kernel_aspace(|aspace| {
                let phys_range = core::ops::Range::from_start_len(
                    PhysicalAddress::new(addr), page_size,
                );
                let m = crate::mem::Mmap::new_phys(
                    aspace.clone(), phys_range, page_size, page_size,
                    Some("virtio-probe".to_string()),
                )?;
                m.commit(aspace.lock().deref_mut(), 0..page_size, true)?;
                Ok::<_, anyhow::Error>(m)
            })?;
            let virt = mmap.range().start.get();
            let magic = unsafe { ptr::read_volatile((virt + VIRTIO_MMIO_MAGIC) as *const u32) };
            let dev_id = unsafe { ptr::read_volatile((virt + VIRTIO_MMIO_DEVICE_ID) as *const u32) };

            if magic == VIRTIO_MAGIC && dev_id == VIRTIO_DEV_INPUT {
                input_phys = addr;
                tracing::info!("Found virtio-input at phys {:#x}", addr);
                break;
            }
            // mmap drops here, unmapping the temporary probe
        }

        if input_phys == 0 {
            anyhow::bail!("No virtio-input device found");
        }

        // Map the device permanently
        let mmap = crate::mem::with_kernel_aspace(|aspace| {
            let phys_range = core::ops::Range::from_start_len(
                PhysicalAddress::new(input_phys), page_size,
            );
            let m = crate::mem::Mmap::new_phys(
                aspace.clone(), phys_range, page_size, page_size,
                Some("virtio-kbd".to_string()),
            )?;
            m.commit(aspace.lock().deref_mut(), 0..page_size, true)?;
            Ok::<_, anyhow::Error>(m)
        })?;
        let base = mmap.range().start.get();
        core::mem::forget(mmap);

        let version = unsafe { ptr::read_volatile((base + VIRTIO_MMIO_VERSION) as *const u32) };
        tracing::info!("VirtIO input: version={}, initializing...", version);

        // Reset
        Self::write_reg(base, VIRTIO_MMIO_STATUS, 0);
        fence(Ordering::SeqCst);

        // Acknowledge + Driver
        Self::write_reg(base, VIRTIO_MMIO_STATUS, VIRTIO_STATUS_ACK);
        Self::write_reg(base, VIRTIO_MMIO_STATUS, VIRTIO_STATUS_ACK | VIRTIO_STATUS_DRIVER);

        // Features (accept defaults for input device)
        if version >= 2 {
            Self::write_reg(base, VIRTIO_MMIO_STATUS,
                VIRTIO_STATUS_ACK | VIRTIO_STATUS_DRIVER | VIRTIO_STATUS_FEATURES_OK);
            let status = unsafe { ptr::read_volatile((base + VIRTIO_MMIO_STATUS) as *const u32) };
            if status & VIRTIO_STATUS_FEATURES_OK == 0 {
                anyhow::bail!("VirtIO features negotiation failed");
            }
        }

        // Select queue 0 (eventq)
        Self::write_reg(base, VIRTIO_MMIO_QUEUE_SEL, 0);
        fence(Ordering::SeqCst);
        let max_q = unsafe { ptr::read_volatile((base + VIRTIO_MMIO_QUEUE_NUM_MAX) as *const u32) };
        let qsz = (QUEUE_SIZE as u32).min(max_q);
        let qsz16 = qsz as u16;
        tracing::info!("Queue 0: max={}, using={}", max_q, qsz);

        Self::write_reg(base, VIRTIO_MMIO_QUEUE_NUM, qsz);

        // Allocate virtqueue memory
        let desc_size = qsz as usize * 16;
        let avail_size = 6 + 2 * qsz as usize;
        let used_size = 6 + 8 * qsz as usize;

        if version == 1 {
            // Legacy: single contiguous allocation with alignment
            Self::write_reg(base, VIRTIO_MMIO_GUEST_PAGE_SIZE, page_size as u32);
            let align = page_size;
            let total = desc_size + avail_size;
            let total_aligned = (total + align - 1) & !(align - 1);
            let full_size = total_aligned + used_size;
            let full_aligned = (full_size + page_size - 1) & !(page_size - 1);

            let queue_buf = vec![0u8; full_aligned + page_size]; // extra for alignment
            let queue_base = queue_buf.as_ptr() as usize;
            let queue_aligned = (queue_base + page_size - 1) & !(page_size - 1);
            let queue_phys = queue_aligned - phys_offset;

            let pfn = (queue_phys / page_size) as u32;
            Self::write_reg(base, VIRTIO_MMIO_QUEUE_ALIGN, align as u32);
            Self::write_reg(base, VIRTIO_MMIO_QUEUE_PFN, pfn);

            let descs = queue_aligned as *mut VirtqDesc;
            let avail_base = queue_aligned + desc_size;
            let used_base = queue_aligned + total_aligned;

            // Allocate event buffers
            let event_buf = vec![0u8; qsz as usize * EVENT_SIZE];
            let events_base = event_buf.as_ptr() as *mut u8;

            // Fill descriptors with event buffers
            for i in 0..qsz as usize {
                let ev_phys = (events_base as usize + i * EVENT_SIZE) - phys_offset;
                unsafe {
                    let desc = &mut *descs.add(i);
                    desc.addr = ev_phys as u64;
                    desc.len = EVENT_SIZE as u32;
                    desc.flags = 2; // VIRTQ_DESC_F_WRITE
                    desc.next = 0;
                }
            }

            // Fill available ring
            let avail_flags = avail_base as *mut u16;
            let avail_idx = unsafe { avail_flags.add(1) };
            let avail_ring = unsafe { avail_flags.add(2) };
            unsafe {
                ptr::write_volatile(avail_flags, 1); // NO_INTERRUPT
                for i in 0..qsz as u16 {
                    ptr::write_volatile(avail_ring.add(i as usize), i);
                }
                ptr::write_volatile(avail_idx, qsz as u16);
            }
            fence(Ordering::Release);

            let used_flags = used_base as *const u16;
            let used_idx_ptr = unsafe { used_flags.add(1) } as *const u16;
            let used_ring_ptr = unsafe { used_flags.add(2) } as *const [u32; 2];

            // Driver OK
            Self::write_reg(base, VIRTIO_MMIO_STATUS,
                VIRTIO_STATUS_ACK | VIRTIO_STATUS_DRIVER | VIRTIO_STATUS_DRIVER_OK);
            fence(Ordering::SeqCst);

            // Notify queue 0
            Self::write_reg(base, VIRTIO_MMIO_QUEUE_NOTIFY, 0);

            tracing::info!("VirtIO keyboard initialized (legacy transport)");

            Ok(Self {
                base,
                descs,
                avail_flags,
                avail_idx,
                avail_ring,
                used_flags,
                used_idx: used_idx_ptr,
                used_ring: used_ring_ptr,
                events_base,
                last_seen_used: 0,
                buttons: 0,
                quit: false,
                _queue_buf: queue_buf,
                _event_buf: event_buf,
            })
        } else {
            // Modern (v2) transport
            let queue_buf = vec![0u8; desc_size + avail_size + used_size + 4096];
            let qb = queue_buf.as_ptr() as usize;

            let desc_virt = (qb + 15) & !15; // 16-byte aligned
            let avail_virt = desc_virt + desc_size;
            let used_virt = (avail_virt + avail_size + 3) & !3; // 4-byte aligned

            let desc_phys = desc_virt - phys_offset;
            let avail_phys = avail_virt - phys_offset;
            let used_phys = used_virt - phys_offset;

            Self::write_reg(base, VIRTIO_MMIO_QUEUE_DESC_LOW, desc_phys as u32);
            Self::write_reg(base, VIRTIO_MMIO_QUEUE_DESC_HIGH, (desc_phys >> 32) as u32);
            Self::write_reg(base, VIRTIO_MMIO_QUEUE_DRIVER_LOW, avail_phys as u32);
            Self::write_reg(base, VIRTIO_MMIO_QUEUE_DRIVER_HIGH, (avail_phys >> 32) as u32);
            Self::write_reg(base, VIRTIO_MMIO_QUEUE_DEVICE_LOW, used_phys as u32);
            Self::write_reg(base, VIRTIO_MMIO_QUEUE_DEVICE_HIGH, (used_phys >> 32) as u32);
            Self::write_reg(base, VIRTIO_MMIO_QUEUE_READY, 1);

            let event_buf = vec![0u8; qsz as usize * EVENT_SIZE];
            let events_base = event_buf.as_ptr() as *mut u8;
            let descs = desc_virt as *mut VirtqDesc;

            for i in 0..qsz as usize {
                let ev_phys = (events_base as usize + i * EVENT_SIZE) - phys_offset;
                unsafe {
                    let desc = &mut *descs.add(i);
                    desc.addr = ev_phys as u64;
                    desc.len = EVENT_SIZE as u32;
                    desc.flags = 2;
                    desc.next = 0;
                }
            }

            let avail_flags = avail_virt as *mut u16;
            let avail_idx_ptr = unsafe { avail_flags.add(1) };
            let avail_ring_ptr = unsafe { avail_flags.add(2) };
            unsafe {
                ptr::write_volatile(avail_flags, 1);
                for i in 0..qsz as u16 {
                    ptr::write_volatile(avail_ring_ptr.add(i as usize), i);
                }
                ptr::write_volatile(avail_idx_ptr, qsz as u16);
            }
            fence(Ordering::Release);

            let used_flags = used_virt as *const u16;
            let used_idx_ptr = unsafe { used_flags.add(1) } as *const u16;
            let used_ring_ptr = unsafe { used_flags.add(2) } as *const [u32; 2];

            Self::write_reg(base, VIRTIO_MMIO_STATUS,
                VIRTIO_STATUS_ACK | VIRTIO_STATUS_DRIVER | VIRTIO_STATUS_FEATURES_OK | VIRTIO_STATUS_DRIVER_OK);
            fence(Ordering::SeqCst);
            Self::write_reg(base, VIRTIO_MMIO_QUEUE_NOTIFY, 0);

            tracing::info!("VirtIO keyboard initialized (modern transport)");

            Ok(Self {
                base,
                descs,
                avail_flags,
                avail_idx: avail_idx_ptr,
                avail_ring: avail_ring_ptr,
                used_flags,
                used_idx: used_idx_ptr,
                used_ring: used_ring_ptr,
                events_base,
                last_seen_used: 0,
                buttons: 0,
                quit: false,
                _queue_buf: queue_buf,
                _event_buf: event_buf,
            })
        }
    }

    fn write_reg(base: usize, offset: usize, val: u32) {
        unsafe { ptr::write_volatile((base + offset) as *mut u32, val); }
    }

    pub fn poll_events(&mut self) {
        // ACK any pending interrupts
        let isr = unsafe { ptr::read_volatile((self.base + 0x060) as *const u32) };
        if isr != 0 {
            unsafe { ptr::write_volatile((self.base + VIRTIO_MMIO_INTERRUPT_ACK) as *mut u32, isr); }
        }

        fence(Ordering::Acquire);
        loop {
            let used_idx = unsafe { ptr::read_volatile(self.used_idx) };
            if self.last_seen_used == used_idx {
                break;
            }

            let ring_idx = (self.last_seen_used % QUEUE_SIZE) as usize;
            let used_elem = unsafe { ptr::read_volatile(self.used_ring.add(ring_idx)) };
            let desc_idx = used_elem[0] as usize;

            // Read event from buffer
            let ev_ptr = unsafe { self.events_base.add(desc_idx * EVENT_SIZE) };
            let ev_type = unsafe { ptr::read_volatile(ev_ptr as *const u16) };
            let ev_code = unsafe { ptr::read_volatile(ev_ptr.add(2) as *const u16) };
            let ev_value = unsafe { ptr::read_volatile(ev_ptr.add(4) as *const u32) };

            if ev_type == EV_KEY {
                let bit = keycode_to_button(ev_code);
                if ev_code == KEY_Q && ev_value == 1 {
                    self.quit = true;
                } else if let Some(b) = bit {
                    if ev_value == 1 {
                        self.buttons |= b;
                    } else if ev_value == 0 {
                        self.buttons &= !b;
                    }
                }
            }

            // Recycle descriptor back to available ring
            let avail_idx = unsafe { ptr::read_volatile(self.avail_idx) };
            let avail_ring_idx = (avail_idx % QUEUE_SIZE) as usize;
            unsafe { ptr::write_volatile(self.avail_ring.add(avail_ring_idx), desc_idx as u16); }
            fence(Ordering::Release);
            unsafe { ptr::write_volatile(self.avail_idx, avail_idx.wrapping_add(1)); }
            fence(Ordering::Release);

            self.last_seen_used = self.last_seen_used.wrapping_add(1);
        }

        // Notify device that we recycled descriptors
        Self::write_reg(self.base, VIRTIO_MMIO_QUEUE_NOTIFY, 0);
    }

    pub fn buttons(&self) -> u16 {
        self.buttons
    }

    pub fn quit_requested(&self) -> bool {
        self.quit
    }

    pub fn clear_quit(&mut self) {
        self.quit = false;
    }
}

fn keycode_to_button(code: u16) -> Option<u16> {
    match code {
        KEY_UP | KEY_W => Some(1 << 0),
        KEY_DOWN | KEY_S => Some(1 << 1),
        KEY_LEFT | KEY_A => Some(1 << 2),
        KEY_RIGHT | KEY_D => Some(1 << 3),
        KEY_SPACE | KEY_Z => Some(1 << 4),
        KEY_ENTER => Some(1 << 8),
        _ => None,
    }
}
