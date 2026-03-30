use core::fmt::Write;
use spin::Mutex;

pub static LOGGER: UartLogger = UartLogger;

static WRITER: Mutex<UartWriter> = Mutex::new(UartWriter);

struct UartWriter;

pub fn write_byte(byte: u8) {
    let w = UartWriter;
    w.do_write_byte(byte);
}

impl UartWriter {
    fn do_write_byte(&self, byte: u8) {
        #[cfg(target_arch = "x86_64")]
        unsafe {
            // COM1: 0x3F8 - wait for TX ready then send
            core::arch::asm!(
                "2: in al, dx",
                "test al, 0x20",
                "jz 2b",
                "mov al, {0}",
                "mov dx, 0x3F8",
                "out dx, al",
                in(reg_byte) byte,
                out("al") _,
                out("dx") _,
            );
        }

        #[cfg(target_arch = "aarch64")]
        unsafe {
            // PL011 UART on QEMU virt: 0x0900_0000
            let uart_base: *mut u8 = 0x0900_0000 as *mut u8;
            core::ptr::write_volatile(uart_base, byte);
        }
    }
}

impl Write for UartWriter {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        for byte in s.bytes() {
            if byte == b'\n' {
                self.do_write_byte(b'\r');
            }
            self.do_write_byte(byte);
        }
        Ok(())
    }
}

pub fn early_print(s: &str) {
    let writer = UartWriter;
    for byte in s.bytes() {
        writer.do_write_byte(byte);
    }
}

pub fn init() {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        // Initialize COM1
        core::arch::asm!(
            "mov dx, 0x3F9", "xor al, al", "out dx, al",  // Disable interrupts
            "mov dx, 0x3FB", "mov al, 0x80", "out dx, al", // DLAB
            "mov dx, 0x3F8", "mov al, 0x01", "out dx, al", // Baud 115200
            "mov dx, 0x3F9", "xor al, al", "out dx, al",
            "mov dx, 0x3FB", "mov al, 0x03", "out dx, al", // 8N1
            "mov dx, 0x3FC", "mov al, 0x03", "out dx, al", // RTS/DSR
            out("al") _,
            out("dx") _,
        );
    }
    // AArch64 PL011 on QEMU virt is usable without init
}

pub struct UartLogger;

impl log::Log for UartLogger {
    fn enabled(&self, _metadata: &log::Metadata) -> bool {
        true
    }

    fn log(&self, record: &log::Record) {
        let mut writer = WRITER.lock();
        let _ = writeln!(writer, "[{}] {}", record.level(), record.args());
    }

    fn flush(&self) {}
}
