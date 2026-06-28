//! PL011 UART0 output driver for QEMU `-M virt`.
//!
//! Lives in its own module so the `Platform` implementation and early-boot
//! code can both emit characters without circular imports.

/// PL011 UART0 data register on QEMU `-M virt`. Writing a byte transmits it.
const UART0_DR: *mut u8 = 0x0900_0000 as *mut u8;

/// Emit one byte over UART0.
#[inline]
pub fn uart_put(b: u8) {
    unsafe { core::ptr::write_volatile(UART0_DR, b) };
}

/// Emit a string over UART0, expanding `\n` to `\r\n`.
pub fn uart_str(s: &str) {
    for &b in s.as_bytes() {
        if b == b'\n' {
            uart_put(b'\r');
        }
        uart_put(b);
    }
}
