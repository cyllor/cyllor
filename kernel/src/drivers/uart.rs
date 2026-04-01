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
        crate::arch::uart_write_byte(byte);
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

/// Enable UART RX interrupt (call after interrupt controller is initialized)
pub fn enable_rx_interrupt() {
    crate::arch::uart_enable_rx_interrupt();
    log::debug!("UART RX interrupt enabled");
}

/// Handle UART RX interrupt — drain FIFO into PTY
pub fn handle_rx_interrupt() {
    crate::arch::uart_handle_rx_interrupt(crate::drivers::pty::push_uart_byte);
}

pub fn init() {
    crate::arch::uart_init();
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
