/* SPDX-License-Identifier: MPL-2.0 */

    .section .text

    .global trap_threadlet_vector_base
    .align 8
trap_threadlet_vector_base:
    j trap_threadlet_entry                     # Direct-mode trap entry

    .global trap_threadlet_entry
    .align 8
trap_threadlet_entry:
    li      t1, 5
    li      t2, 101
    .insn   r 0x0B, 0, 0xe, x0, t1, t2

    addi    sp, sp, -2 * XLENB                 
    # We allow nested interrupts in the softirq.
    csrr    t1, sstatus
    sd      t1, 0(sp)                          # preserve sstatus only

    mv      a0, sp                             # trap_handler_interrupt ignores contents
    la      ra, trap_threadlet_interrupt_return
    call    trap_handler_threadlet_interrupt

trap_threadlet_interrupt_return:
    ld      t1, 0(sp)                          # restore sstatus
    csrw    sstatus, t1
    addi    sp, sp, 2 * XLENB                  # restore original stack pointer

    # debug again
    li      t1, 5
    li      t2, 12345
    .insn   r 0x0B, 0, 0xe, x0, t1, t2


    .insn   r 0x0b, 0x1, 0x00, x0, x0, x0     # eret_hint
    nop
    sret

    # do not use threadlet yield for now, sret recover CSRs and set this threadlet to be unrunnable
    #.insn   r 0x0B, 0, 0x8, x0, x0, x0         # THREAD_YIELD
    #j       trap_threadlet_vector_base
