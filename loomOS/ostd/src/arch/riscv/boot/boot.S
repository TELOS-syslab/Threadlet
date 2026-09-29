/* SPDX-License-Identifier: MPL-2.0 */

.section .text.entry
.globl _start
_start:
    # Arguments passed from SBI:
    #   a0 = hart id
    #   a1 = device tree paddr (not touched)

    # Install a temporary trap vector that simply issues WFI. This avoids
    # spurious re-entry loops before the Rust trap code replaces stvec.
    la     t0, boot_trap_wfi
    csrw   stvec, t0

    # 1. enable paging
    # setting up 1st pagetable
    #   entry[511] = (PPN(boot_pagetable_2nd) << 10) | 0x01 # V
    la     t1, boot_pagetable
    li     t0, 8 * 256
    add    t1, t1, t0
    la     t0, boot_pagetable_linear
    srli   t0, t0, 2
    ori    t0, t0, 0x01
    sd     t0, 0(t1)

    la     t1, boot_pagetable
    li     t0, 8 * 511
    add    t1, t1, t0
    la     t0, boot_pagetable_2nd
    srli   t0, t0, 2
    ori    t0, t0, 0x01
    sd     t0, 0(t1)

    la     t0, boot_pagetable
    li     t1, 9 << 60
    srli   t0, t0, 12
    or     t0, t0, t1
    csrw   satp, t0
   sfence.vma

    # 1.5 zero .bss section so global statics start clean
    lga    t0, __bss
    lga    t1, __bss_end
    beq    t0, t1, .Lbss_done
.Lbss_loop:
    sb     zero, 0(t0)
    addi   t0, t0, 1
    bltu   t0, t1, .Lbss_loop
.Lbss_done:

    # 2. set sp (BSP only) and enforce 16-byte alignment per RISC-V ABI
    lga    sp, boot_stack_top
    andi   sp, sp, -16

    # 3. jump to rust riscv_boot
    lga    t0, riscv_boot
    jr     t0

.globl boot_trap_wfi
boot_trap_wfi:
    csrr   a0, scause
    csrr   a1, stval
    la     t0, boot_trap_handler
    jr     t0


.section .bss.stack
.p2align 4               # ensure 16-byte alignment for stack region

.globl boot_stack_bottom
boot_stack_bottom:
    .space 0x40000 # 64 KiB

.globl boot_stack_top
boot_stack_top:


.section .data

.align 12

# 1GB phyc mem mapped
boot_pagetable:
    .quad (0x00000 << 10) | 0xcf # VRWXAD
    .zero 8 * 255
    .quad 0  # To-Be-Assign (linear mapping L2 pointer)
    .zero 8 * 254
    .quad 0  # To-Be-Assign

boot_pagetable_2nd:
    # 0x0000_00ff_8000_0000 -> 0x0000_0000_8000_0000
    .zero 8 * 508
    .quad (0x00000 << 10) | 0xcf # VRWXAD
    .quad (0x40000 << 10) | 0xcf # VRWXAD
    .quad (0x80000 << 10) | 0xcf # VRWXAD
    .quad 0

boot_pagetable_linear:
    # Linear mapping L2 entries (1 GiB superpages), cover 0..8 GiB phys
    .quad (0x00000 << 10) | 0xcf # 0 GiB - 1 GiB
    .quad (0x40000 << 10) | 0xcf # 1 GiB - 2 GiB
    .quad (0x80000 << 10) | 0xcf # 2 GiB - 3 GiB
    .quad (0xC0000 << 10) | 0xcf # 3 GiB - 4 GiB
    .quad (0x100000 << 10) | 0xcf # 4 GiB - 5 GiB
    .quad (0x140000 << 10) | 0xcf # 5 GiB - 6 GiB
    .quad (0x180000 << 10) | 0xcf # 6 GiB - 7 GiB
    .quad (0x1C0000 << 10) | 0xcf # 7 GiB - 8 GiB
    .zero 8 * (512 - 8)
