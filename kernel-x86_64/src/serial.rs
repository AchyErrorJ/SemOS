//! Serial Port Driver (16550 UART)
//!
//! x86_64 uses the 16550 UART accessed via I/O ports.
//! COM1 is at I/O port 0x3F8.
//!
//! # Registers (offsets from base)
//!
//! | Offset | DLAB=0 Read    | DLAB=0 Write   | DLAB=1        |
//! |--------|----------------|----------------|---------------|
//! | 0      | RX Buffer      | TX Buffer      | Divisor Low   |
//! | 1      | Int Enable     | Int Enable     | Divisor High  |
//! | 2      | Int ID         | FIFO Control   | -             |
//! | 3      | Line Control   | Line Control   | -             |
//! | 4      | Modem Control  | Modem Control  | -             |
//! | 5      | Line Status    | -              | -             |
//! | 6      | Modem Status   | -              | -             |
//! | 7      | Scratch        | Scratch        | -             |

use core::fmt::{self, Write};
use spin::Mutex;
use x86_64::instructions::port::Port;

/// COM1 base I/O port
const COM1_BASE: u16 = 0x3F8;

/// Line Status Register bits
mod line_status {
    pub const DATA_READY: u8 = 1 << 0;
    pub const TRANSMIT_EMPTY: u8 = 1 << 5;
}

/// Serial port structure
pub struct SerialPort {
    data: Port<u8>,         // Offset 0: Data register
    int_enable: Port<u8>,   // Offset 1: Interrupt enable
    fifo_ctrl: Port<u8>,    // Offset 2: FIFO control
    line_ctrl: Port<u8>,    // Offset 3: Line control
    modem_ctrl: Port<u8>,   // Offset 4: Modem control
    line_status: Port<u8>,  // Offset 5: Line status
}

impl SerialPort {
    /// Create a new serial port at the given base address.
    pub const fn new(base: u16) -> Self {
        Self {
            data: Port::new(base),
            int_enable: Port::new(base + 1),
            fifo_ctrl: Port::new(base + 2),
            line_ctrl: Port::new(base + 3),
            modem_ctrl: Port::new(base + 4),
            line_status: Port::new(base + 5),
        }
    }

    /// Initialize the serial port.
    ///
    /// Sets up 115200 baud, 8N1 (8 data bits, no parity, 1 stop bit).
    pub fn init(&mut self) {
        unsafe {
            // Disable interrupts
            self.int_enable.write(0x00);

            // Enable DLAB (set baud rate divisor)
            self.line_ctrl.write(0x80);

            // Set divisor to 1 (115200 baud)
            // Divisor = 115200 / baud_rate
            // For 115200: divisor = 1
            self.data.write(0x01);      // Divisor low byte
            self.int_enable.write(0x00); // Divisor high byte

            // 8 bits, no parity, one stop bit (8N1)
            // Also clears DLAB
            self.line_ctrl.write(0x03);

            // Enable FIFO, clear them, 14-byte threshold
            self.fifo_ctrl.write(0xC7);

            // IRQs enabled, RTS/DSR set
            self.modem_ctrl.write(0x0B);

            // Enable receive interrupts (optional, we poll for now)
            // self.int_enable.write(0x01);
        }
    }

    /// Check if the transmit buffer is empty.
    fn is_transmit_empty(&mut self) -> bool {
        unsafe { self.line_status.read() & line_status::TRANSMIT_EMPTY != 0 }
    }

    /// Check if data is ready to be read.
    fn is_data_ready(&mut self) -> bool {
        unsafe { self.line_status.read() & line_status::DATA_READY != 0 }
    }

    /// Write a byte to the serial port.
    pub fn write_byte(&mut self, byte: u8) {
        // Wait for transmit buffer to be empty
        while !self.is_transmit_empty() {
            core::hint::spin_loop();
        }

        unsafe {
            self.data.write(byte);
        }
    }

    /// Read a byte from the serial port (blocking).
    pub fn read_byte(&mut self) -> u8 {
        // Wait for data to be ready
        while !self.is_data_ready() {
            core::hint::spin_loop();
        }

        unsafe { self.data.read() }
    }

    /// Try to read a byte (non-blocking).
    pub fn try_read_byte(&mut self) -> Option<u8> {
        if self.is_data_ready() {
            Some(unsafe { self.data.read() })
        } else {
            None
        }
    }

    /// Write a string to the serial port.
    pub fn write_str(&mut self, s: &str) {
        for byte in s.bytes() {
            self.write_byte(byte);
        }
    }
}

impl Write for SerialPort {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.write_str(s);
        Ok(())
    }
}

/// Global serial port instance
pub static SERIAL: Mutex<SerialPort> = Mutex::new(SerialPort::new(COM1_BASE));

/// Initialize the serial port.
pub fn init() {
    SERIAL.lock().init();
}

/// Print to serial port (used by print! macro). Also mirrors output to the
/// framebuffer console once it's been initialized.
#[doc(hidden)]
pub fn _print(args: fmt::Arguments) {
    use core::fmt::Write;
    // Disable interrupts while printing to avoid deadlock
    x86_64::instructions::interrupts::without_interrupts(|| {
        // Mirror into the netlog ring first. This is a pure byte copy — no net,
        // no alloc, no printing — so it's safe here with interrupts off and the
        // serial lock held. The actual UDP send happens later in SYS_NETLOG
        // (normal syscall context), never from inside _print (that would
        // recurse/deadlock: net::poll prints status).
        crate::netlog::mirror(args);
        SERIAL.lock().write_fmt(args).unwrap();
        // Mirror to framebuffer (no-op if not yet initialized). Skip only when
        // a fullscreen kernel app (agent/editor) owns the screen;
        // `SUPPRESS_TTY_INPUT` is for keyboard echo only, so user-space output
        // (shell prompts, command output, rustc diagnostics) stays visible.
        if !crate::FULLSCREEN_APP_ACTIVE.load(core::sync::atomic::Ordering::Relaxed) {
            crate::framebuffer::_print(args);
        }
    });
}

/// Simple serial wrapper for direct access (like ARM64 Uart)
pub struct Serial;

impl Serial {
    pub fn puts(s: &str) {
        _print(format_args!("{}", s));
    }

    pub fn put_char(c: char) {
        SERIAL.lock().write_byte(c as u8);
    }

    pub fn put_hex(val: u64) {
        _print(format_args!("{:016X}", val));
    }

    pub fn put_num(val: u64) {
        _print(format_args!("{}", val));
    }

    pub fn getc() -> Option<u8> {
        SERIAL.lock().try_read_byte()
    }
}
