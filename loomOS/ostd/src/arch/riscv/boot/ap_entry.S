.section .text.trampoline
.global _ap_startup
    .extern boot_trap_wfi

# AP启动入口点
# ABI:
#   a0: hartid
#   a1: opaque value (trampoline物理地址)

_ap_startup:
    mv      sp, a1                 # tmp set stack pointer

    addi    t2, sp, -16
    ld      t3, 0(t2)            # t3 = satp_value

    addi    t2, sp, -8
    ld      t5, 0(t2)           # t5 = ap_early_entry_vaddr

    addi    t2, sp, -24
    ld      t6, 0(t2)           # stack vaddr 
   
 
    addi    t2, sp, -32           # the addr we write test flag
    mv      t4, t6
    add     t4, t4, a0              # t4 = stack vaddr + hartid
    sd      t4, 0(t2)               # write flag


    csrw    satp, t3
    sfence.vma

    # Warning: we have enabled paging but the pc is still physical addr.
    mv      sp, t6

    # Initialize GP for small-data accesses before entering Rust.
    # Rationale:
    # - The kernel and compiler may emit GP-relative accesses for .sdata/.sbss
    #   before cpu::local::init_on_ap() switches GP to per-CPU storage.
    # Implementation details:
    # - Guard with `.option norelax` to prevent linker relaxation from turning
    #   it into a PC-relative sequence.
    .option push
    .option norelax
    lga     gp, __global_pointer$
    .option pop
    
    # Set S-mode trap vector for AP same as BSP: route to boot_trap_handler
    # via boot_trap_wfi stub which passes scause/stval in a0/a1.
    lga     t0, boot_trap_wfi
    csrw    stvec, t0
    addi    t2, sp, -2000
    li      t4, 0xAC00 
    add     t4, t4, a0
    sd      t4, 0(t2)               # write again

    lga    t0, ap_early_entry  
    jr     t0

    # jr t5               # jmp to ap_early_entry 
    

    # should not enter here
loop:
    wfi
    j       loop


.section .bss.apstack

.globl ap_stack_bottom
ap_stack_bottom:
    .space 0x40000 # 64 KiB
    .space 0x40000 # 64 KiB
    .space 0x40000 # 64 KiB
    .space 0x40000 # 64 KiB
    .space 0x40000 # 64 KiB
    .space 0x40000 # 64 KiB
    .space 0x40000 # 64 KiB
    .space 0x40000 # 64 KiB

.globl ap_stack_top
ap_stack_top:
