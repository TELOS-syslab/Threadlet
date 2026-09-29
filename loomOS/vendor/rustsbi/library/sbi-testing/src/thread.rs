#[doc = " 线程上下文。"]
#[repr(C)]
pub struct Thread {
    sctx: usize,
    x: [usize; 31],
    sepc: usize,
}

#[allow(unused)]
impl Thread {
    #[doc = " 创建空白上下文。"]
    #[inline]
    pub const fn new(sepc: usize) -> Self {
        Self {
            sctx: 0,
            x: [0; 31],
            sepc,
        }
    }

    #[doc = " 读取通用寄存器。"]
    #[inline]
    pub fn x(&self, n: usize) -> usize {
        self.x[n - 1]
    }

    #[doc = " 修改通用寄存器。"]
    #[inline]
    pub fn x_mut(&mut self, n: usize) -> &mut usize {
        &mut self.x[n - 1]
    }

    #[doc = " 读取参数寄存器。"]
    #[inline]
    pub fn a(&self, n: usize) -> usize {
        self.x(n + 10)
    }

    #[doc = " 修改参数寄存器。"]
    #[inline]
    pub fn a_mut(&mut self, n: usize) -> &mut usize {
        self.x_mut(n + 10)
    }

    #[doc = " 读取栈指针。"]
    #[inline]
    pub fn sp(&self) -> usize {
        self.x(2)
    }

    #[doc = " 修改栈指针。"]
    #[inline]
    pub fn sp_mut(&mut self) -> &mut usize {
        self.x_mut(2)
    }

    #[doc = " 将 pc 移至下一条指令。"]
    ///
    /// # Notice
    ///
    #[doc = " 假设这一条指令不是压缩版本。"]
    #[inline]
    pub fn move_next(&mut self) {
        self.sepc = self.sepc.wrapping_add(4);
    }

    #[doc = " 执行此线程，并返回 `sstatus`。"]
    ///
    /// # Safety
    ///
    #[doc = " 将修改 `sscratch`、`sepc`、`sstatus` 和 `stvec`。"]
    #[inline]
    pub unsafe fn execute(&mut self) -> usize {
        unsafe {

            let mut sstatus: usize;
            core::arch::asm!("csrr {}, sstatus", out(reg) sstatus);
            const PRIVILEGE_BIT: usize = 1 << 8;
            const INTERRUPT_BIT: usize = 1 << 5;
            sstatus |= PRIVILEGE_BIT | INTERRUPT_BIT;


            core::arch::asm!(
                "   csrw sscratch, {sscratch}
                csrw sepc    , {sepc}
                csrw sstatus , {sstatus}
                addi sp, sp, -8
                sd   ra, (sp)
                call {execute_naked}
                ld   ra, (sp)
                addi sp, sp,  8
                csrr {sepc}   , sepc
                csrr {sstatus}, sstatus
            ",
                sscratch      = in(reg) self,
                sepc          = inlateout(reg) self.sepc,
                sstatus       = inlateout(reg) sstatus,
                execute_naked = sym execute_naked,
            );
            sstatus
        }
    }
}

#[doc = " 线程切换核心部分。"]
///
#[doc = " 通用寄存器压栈，然后从预存在 `sscratch` 里的上下文指针恢复线程通用寄存器。"]
///
/// # Safety
///
#[doc = " 裸函数。"]
#[unsafe(naked)]
unsafe extern "C" fn execute_naked() {
    core::arch::naked_asm!(
        r"  .altmacro
        .macro SAVE n
            sd x\n, \n*8(sp)
        .endm
        .macro SAVE_ALL
            sd x1, 1*8(sp)
            .set n, 3
            .rept 29
                SAVE %n
                .set n, n+1
            .endr
        .endm

        .macro LOAD n
            ld x\n, \n*8(sp)
        .endm
        .macro LOAD_ALL
            ld x1, 1*8(sp)
            .set n, 3
            .rept 29
                LOAD %n
                .set n, n+1
            .endr
        .endm
    ",

        "   .option push
        .option nopic
    ",

        "   addi sp, sp, -32*8
        SAVE_ALL
    ",

        "   la   t0, 2f
        csrw stvec, t0
    ",

        "   csrr t0, sscratch
        sd   sp, (t0)
        mv   sp, t0
    ",

        "   LOAD_ALL
        ld   sp, 2*8(sp)
    ",

        "   sret",

        "   .align 2",

        "2: csrrw sp, sscratch, sp",

        "   SAVE_ALL
        csrrw t0, sscratch, sp
        sd    t0, 2*8(sp)
    ",

        "   ld sp, (sp)",

        "   LOAD_ALL
        addi sp, sp, 32*8
    ",

        "   ret",
        "   .option pop",
    )
}
