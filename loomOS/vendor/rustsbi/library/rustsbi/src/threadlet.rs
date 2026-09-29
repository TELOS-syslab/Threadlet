use sbi_rt;
use crate::LOGO; 
use core::fmt::Write; 


struct Console;

impl Write for Console {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        for c in s.chars() {
            sbi_rt::legacy::console_putchar(c as usize);
        }
        Ok(())
    }
}

#[macro_export]
macro_rules! threadlet_print {
    ($($arg:tt)*) => ({
        use core::fmt::Write;
        Console.write_fmt(format_args!($($arg)*)).unwrap();
    });
}


pub fn print_hello() -> sbi_rt::SbiRet {
    threadlet_print!("{}", LOGO); 
    threadlet_print!("[Threadlet] Running on RISC-V architecture with FDT support\n");
    sbi_rt::SbiRet::ok(0) 
} 