// drivers/pci.rs — PCIe ECAM (Enhanced Configuration Access Mechanism) enumeration
//
// QEMU virt (AArch64): PCIe ECAM base = 0x4010_0000_0000
// Config space address for (bus, dev, func, reg):
//   base + (bus << 20) | (dev << 15) | (func << 12) | (reg & 0xFFC)
//
// x86_64: ECAM base comes from ACPI MCFG table (not yet implemented).

/// Parsed PCI device descriptor.
#[derive(Debug, Clone)]
pub struct PciDevice {
    pub bus: u8,
    pub dev: u8,
    pub func: u8,
    pub vendor_id: u16,
    pub device_id: u16,
    pub class: u8,
    pub subclass: u8,
    pub header_type: u8,
}

#[cfg(target_arch = "aarch64")]
mod ecam {
    use super::PciDevice;

    /// PCIe ECAM physical base on QEMU virt AArch64.
    const ECAM_PHYS_BASE: u64 = 0x4010_0000_0000;

    const MAX_BUS: u8 = 1;
    const MAX_DEV: u8 = 32;
    const MAX_FUNC: u8 = 8;

    unsafe fn ecam_read32(base_virt: u64, bus: u8, dev: u8, func: u8, offset: u16) -> u32 {
        let addr = base_virt
            | ((bus as u64) << 20)
            | ((dev as u64) << 15)
            | ((func as u64) << 12)
            | (offset as u64 & 0xFFC);
        unsafe { core::ptr::read_volatile(addr as *const u32) }
    }

    pub fn enumerate() -> alloc::vec::Vec<PciDevice> {
        let mut devices = alloc::vec::Vec::new();
        let ecam_virt = ECAM_PHYS_BASE + crate::arch::hhdm_offset();

        for bus in 0..MAX_BUS {
            for dev in 0..MAX_DEV {
                for func in 0..MAX_FUNC {
                    let id = unsafe { ecam_read32(ecam_virt, bus, dev, func, 0x00) };
                    let vendor_id = (id & 0xFFFF) as u16;
                    let device_id = (id >> 16) as u16;

                    if vendor_id == 0xFFFF {
                        if func == 0 { break; }
                        continue;
                    }

                    let class_dword = unsafe { ecam_read32(ecam_virt, bus, dev, func, 0x08) };
                    let class    = (class_dword >> 24) as u8;
                    let subclass = (class_dword >> 16) as u8;

                    let hdr_dword = unsafe { ecam_read32(ecam_virt, bus, dev, func, 0x0C) };
                    let header_type = (hdr_dword >> 16) as u8;

                    devices.push(PciDevice { bus, dev, func, vendor_id, device_id, class, subclass, header_type });

                    if func == 0 && (header_type & 0x80) == 0 { break; }
                }
            }
        }
        devices
    }

    pub fn init() {
        let devs = enumerate();
        if devs.is_empty() {
            log::debug!("PCI: no devices found (ECAM base 0x{:x})", ECAM_PHYS_BASE);
            return;
        }
        log::info!("PCI: {} device(s) found:", devs.len());
        for d in &devs {
            log::info!(
                "  [{:02x}:{:02x}.{:x}] vendor={:04x} device={:04x} class={:02x}/{:02x}",
                d.bus, d.dev, d.func, d.vendor_id, d.device_id, d.class, d.subclass,
            );
        }
    }
}

/// Enumerate all PCI devices on the bus and return the list.
pub fn enumerate() -> alloc::vec::Vec<PciDevice> {
    #[cfg(target_arch = "aarch64")]
    return ecam::enumerate();
    #[cfg(not(target_arch = "aarch64"))]
    return alloc::vec::Vec::new(); // TODO: parse ACPI MCFG on x86_64
}

/// Enumerate PCI devices and log them.
pub fn init() {
    #[cfg(target_arch = "aarch64")]
    ecam::init();
}
