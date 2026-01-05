//！ 引入汇编代码

#![no_std]
#![feature(slice_ptr_get)]
#![feature(get_mut_unchecked)]
#![feature(allocator_api)]
#![feature(alloc_error_handler)]
#![allow(dead_code)]
#![warn(rust_2018_idioms)]
#![feature(new_zeroed_alloc)]

use core::arch::global_asm;

#[macro_use]
extern crate bitflags;

extern crate alloc;

global_asm!(include_str!("asm/entry.S"));
global_asm!(include_str!("asm/kernelvec.S"));
global_asm!(include_str!("asm/swtch.S"));
global_asm!(include_str!("asm/trampoline.S"));

#[macro_use]
mod printf;

mod consts;
mod fs;
mod mm;
mod process;
mod register;
mod rmain;
mod spinlock;
mod sleeplock;
mod start;
mod trap;
mod driver;
mod plic;

#[cfg(feature = "unit_test")]
fn test_main_entry() {
    use proc::cpu_id;

    let cpu_id = unsafe { cpu_id() };

    // 只需要在单个硬件线程 / 内核线程上执行的测试用例
    if cpu_id == 0 {
        spinlock::tests::smoke();
    }

    // 需要在多个硬件线程 / 内核线程上执行的测试用例
    printf::tests::println_simo();
    mm::kalloc::tests::alloc_simo();

    if cpu_id == 0 {
        println!("all tests pass.");
    }
}
