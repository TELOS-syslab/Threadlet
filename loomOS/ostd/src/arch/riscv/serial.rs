// SPDX-License-Identifier: MPL-2.0

//! The console I/O.
//!
//! RISC-V output currently uses SBI legacy `console_write_byte` for
//! simplicity and early bring-up. For input on Rocket/SiFive platforms
//! (e.g., `sifive,uart0`), we wire the UART RX watermark interrupt via
//! PLIC and deliver bytes to registered callbacks.

use alloc::{fmt, vec::Vec};
use core::fmt::Write;
use spin::Once;

use crate::{arch::boot::DEVICE_TREE, io_mem::IoMem, mm::VmIoOnce, sync::SpinLock, trap::TrapFrame};
use fdt::node::FdtNode;

/// Prints the formatted arguments to the standard output using the serial port.
#[inline]
pub fn print(args: fmt::Arguments) {
    Stdout.write_fmt(args).unwrap();
}

/// The callback function for console input.
pub type InputCallback = dyn Fn(u8) + Send + Sync + 'static;

/// Registers a callback function to be called when there is console input.
pub fn register_console_input_callback(f: &'static InputCallback) {
    SERIAL_INPUT_CALLBACKS.lock().push(f);
}

struct Stdout;

impl Write for Stdout {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for &c in s.as_bytes() {
            send(c);
        }
        Ok(())
    }
}

/// Initializes the serial port (early, before heap/PLIC). No-op for RISC-V.
pub(crate) fn init() {}

/// Sends a byte on the serial port.
pub fn send(data: u8) {
    sbi_rt::console_write_byte(data);
}

// ===== Implementation details for SiFive UART input =====

// SiFive UART register offsets (bytes) and bits, matching QEMU sifive_uart.h
const UART_TXFIFO: usize = 0x00;
const UART_RXFIFO: usize = 0x04;
const UART_TXCTRL: usize = 0x08;
const UART_RXCTRL: usize = 0x0C;
const UART_IE: usize = 0x10;
const UART_IP: usize = 0x14;
const UART_DIV: usize = 0x18;

// IE/IP bits
const UART_IE_TXWM: u32 = 1; // Transmit watermark interrupt enable
const UART_IE_RXWM: u32 = 2; // Receive watermark interrupt enable
const UART_IP_TXWM: u32 = 1; // Transmit watermark interrupt pending
const UART_IP_RXWM: u32 = 2; // Receive watermark interrupt pending

// RXFIFO returns 0x80000000 when empty in QEMU's SiFive UART device
const UART_RXFIFO_EMPTY_MASK: u32 = 0x8000_0000;

static UART_IOMEM: Once<IoMem> = Once::new();
static UART_IRQ_NUM: Once<u16> = Once::new();
static UART_IRQ: Once<crate::trap::IrqLine> = Once::new();
static SERIAL_INPUT_CALLBACKS: SpinLock<Vec<&'static InputCallback>> = SpinLock::new(Vec::new());

fn handle_uart_irq(_tf: &TrapFrame) {
    let Some(io) = UART_IOMEM.get() else { return };

    // Drain RX FIFO while RX watermark pending
    let mut drained = 0usize;
    loop {
        let ip: u32 = io.read_once(UART_IP).unwrap_or(0);
        if (ip & UART_IP_RXWM) == 0 { break; }

        let val: u32 = io.read_once(UART_RXFIFO).unwrap_or(UART_RXFIFO_EMPTY_MASK);
        if (val & UART_RXFIFO_EMPTY_MASK) != 0 {
            // No more data
            break;
        }
        let ch = (val & 0xff) as u8;
        // Dispatch to all registered callbacks
        for cb in SERIAL_INPUT_CALLBACKS.lock().iter() {
            cb(ch);
        }
        drained += 1;
    }
    if drained > 0 {
        // PRINT: RX interrupt summary for SiFive UART
        // crate::early_println!("[uart] RX IRQ: drained {} byte(s)", drained);
    }
}

/// Complete UART IRQ hookup after IRQ allocator/PLIC are initialized.
pub(crate) fn late_enable() {
    // Extra verbose debug prints to diagnose early bring-up issues on Rocket/FPGA.
    crate::early_println!("[uart] late_enable: enter");
    use crate::arch::device::plic;
    use crate::trap::{IrqLine, TrapFrame};

    // Probe DT for a SiFive UART and set up input if present.
    let fdt = if let Some(f) = DEVICE_TREE.get() {
        crate::early_println!("[uart] late_enable: DEVICE_TREE available");
        f
    } else {
        crate::early_println!("[uart] late_enable: DEVICE_TREE missing; skip");
        return;
    };

    // Find a node with compatible = "sifive,uart0" under /soc, else scan whole tree.
    let uart_node = fdt
        .find_node("/soc")
        .and_then(|soc| {
            soc.children().find(|node| {
                node.compatible()
                    .is_some_and(|compat| compat.all().any(|c| c == "sifive,uart0"))
            })
        })
        .or_else(|| {
            fdt.all_nodes().find(|node| {
                node.compatible()
                    .is_some_and(|compat| compat.all().any(|c| c == "sifive,uart0"))
            })
        });

    if uart_node.is_none() {
        crate::early_println!("[uart] late_enable: no node with compatible 'sifive,uart0' found");
    }
    let Some(uart) = uart_node else { return };

    // Map UART MMIO region once (heap and page tables are ready at this stage)
    if UART_IOMEM.get().is_none() {
        if let Some(mut reg_iter) = uart.reg() {
            if let Some(region) = reg_iter.next() {
                crate::early_println!(
                    "[uart] late_enable: map MMIO paddr=0x{:x} size=0x{:x}",
                    region.starting_address as usize,
                    region.size.unwrap_or(0)
                );
                UART_IOMEM.call_once(|| unsafe {
                    super::device::create_device_io_mem(
                        region.starting_address,
                        region.size.unwrap_or(0x1000),
                    )
                });
            }
        }
    }

    let Some(io) = UART_IOMEM.get() else {
        crate::early_println!("[uart] late_enable: UART_IOMEM not available");
        return;
    };

    // Interrupt number from DT (PLIC source ID)
    if UART_IRQ_NUM.get().is_none() {
        // Try robust parsing: interrupts property (be32), else interrupts-extended matching PLIC phandle
        let mut irq_num: u16 = 0;

        // 1) interrupts as be32 first cell
        if let Some(p) = uart.property("interrupts") {
            let v = p.value;
            if v.len() >= 4 {
                irq_num = u32::from_be_bytes([v[0], v[1], v[2], v[3]]) as u16;
            }
        }

        // 2) interrupts-extended: parse pairs (phandle, spec)
        if irq_num == 0 {
            // Find PLIC node and its phandle to match
            let fdt = DEVICE_TREE.get().unwrap();
            let plic_node = fdt
                .find_node("/soc/plic")
                .or_else(|| {
                    fdt.find_node("/soc").and_then(|soc| {
                        soc.children().find(|node| {
                            node.compatible().is_some_and(|compat| {
                                compat.all().any(|c| c == "riscv,plic0" || c == "sifive,plic-1.0.0")
                            })
                        })
                    })
                });
            let plic_phandle: Option<u32> = plic_node
                .and_then(|n| n.property("phandle").map(|p| p.as_usize().unwrap_or(0) as u32))
                .filter(|&h| h != 0);

            if let Some(p) = uart.property("interrupts-extended") {
                let bytes = p.value;
                // Interpret as sequence of be32 cells: phandle, spec (assume 1 cell spec for PLIC)
                let mut i = 0usize;
                while i + 8 <= bytes.len() {
                    let ph = u32::from_be_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]);
                    let spec = u32::from_be_bytes([
                        bytes[i + 4],
                        bytes[i + 5],
                        bytes[i + 6],
                        bytes[i + 7],
                    ]);
                    if spec != 0 {
                        // If we know PLIC phandle, prefer the matching one; else take first non-zero
                        if plic_phandle.map_or(true, |h| h == ph) {
                            irq_num = spec as u16;
                            break;
                        }
                    }
                    i += 8; // advance by two cells
                }
            }
        }

        // 3) Fallback to QEMU/SiFive default
        if irq_num == 0 {
            crate::early_println!("[uart] UART_IRQ_NUM missing; fallback to default=1");
            irq_num = 1;
        }

        crate::early_println!("[uart] UART_IRQ_NUM get irq_num {}", irq_num);
        UART_IRQ_NUM.call_once(|| irq_num);
    }

    crate::early_println!("[uart] UART_IRQ_NUM get done");
    let Some(&irq_num) = UART_IRQ_NUM.get() else {
        crate::early_println!("[uart] late_enable: no IRQ number from DT");
        return;
    };
    if UART_IRQ.get().is_some() {
        return; // already hooked
    }

    crate::early_println!("[uart] try to write UART");
    // Enable UART receivers/transmitters before enabling interrupts.
    // SiFive UART: RXCTRL.rxen=1, TXCTRL.txen=1; keep default watermarks.
    let _ = io.write_once(UART_RXCTRL, &1u32);
    let _ = io.write_once(UART_TXCTRL, &1u32);
    // unsafe { core::arch::asm!("fence rw, rw", options(nostack)); }
    // Enable RX watermark interrupt; threshold remains default (0)
    crate::early_println!(
        "[uart] late_enable: write IE (RXWM) at off=0x{:x}",
        UART_IE
    );
    let _ = io.write_once(UART_IE, &UART_IE_RXWM);
    // PRINT: device detected and initialized
    crate::early_println!(
        "[uart] SiFive UART detected: paddr=0x{:x} len=0x{:x}; enable RXWM, irq={}",
        io.paddr(),
        io.length(),
        irq_num
    );

    // Enable at PLIC and register handler on this IRQ line.
    crate::early_println!("[uart] late_enable: enable PLIC src {}", irq_num);
    plic::enable_external_interrupt(irq_num);
    crate::early_println!("[uart] late_enable: alloc IrqLine {}", irq_num);
    if let Ok(mut line) = IrqLine::alloc_specific(irq_num as u8) {
        line.on_active(handle_uart_irq);
        UART_IRQ.call_once(|| line);
        crate::early_println!("[uart] late_enable: IrqLine registered");
    } else {
        crate::early_println!("[uart] late_enable: IrqLine alloc failed for {}", irq_num);
    }
}
