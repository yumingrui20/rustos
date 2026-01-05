//! Rust语言入口点，系统启动时的初始点


use core::{arch::asm, convert::Into};

use crate::{consts::{CLINT_MTIMECMP, NCPU}, register::sie};
use crate::register::{
    clint, medeleg, mepc, mhartid, mideleg, mie, mscratch, mstatus, mtvec, satp, tp,
};
use crate::rmain::rust_main;

/// 每个CPU的机器模式上下文存储区
///
/// 该数组为每个CPU核心提供32个usize大小的存储空间，但仅使用前6个元素：
/// - [0..3]：为timervec保存寄存器的空间
/// - [4]：CLINT MTIMECMP寄存器的地址
/// - [5]：定时器中断的期望间隔（周期数）
/// 
/// # 安全性
/// - 使用`static mut`声明，访问需在`unsafe`块中
/// - 索引计算：`offset = 32 * hartid`
static mut MSCRATCH0: [usize; NCPU * 32] = [0; NCPU * 32];

/// RISC-V机器模式入口点
///
/// # 功能说明
/// 该函数是内核启动时每个CPU核心执行的第一个函数（在机器模式下）。
/// 它负责初始化核心状态，设置异常委托，配置定时器中断，
/// 并最终切换到监督者模式跳转到rust_main。
///
/// # 流程解释
/// 1. 设置MPP为监督者模式，使mret后进入监督者模式
/// 2. 设置mepc指向rust_main入口点
/// 3. 禁用分页（初始阶段）
/// 4. 委托所有异常和中断给监督者模式
/// 5. 配置物理内存保护(PMP)
/// 6. 初始化定时器中断
/// 7. 将核心ID(hartid)存储在tp寄存器
/// 8. 执行mret切换到监督者模式
///
/// # 注意事项
/// - 此函数标记为`#[no_mangle]`确保链接器能正确找到入口点
/// - 函数永不返回（-> !）
///
/// # 安全性
/// - 直接操作硬件寄存器，需确保正确配置
/// - 访问全局数组MSCRATCH0需unsafe
#[no_mangle]
pub unsafe fn start() -> ! {
    // 设置mstatus.MPP为监督者模式，确保mret后进入监督者模式
    mstatus::set_mpp(mstatus::MPP::Supervisor);

    // 设置异常返回地址为rust_main
    mepc::write(rust_main as usize);

    // 初始阶段禁用分页（satp=0）
    satp::write(0);

    // 委托所有异常(medeleg)和中断(mideleg)给监督者模式
    medeleg::write(0xffff);
    mideleg::write(0xffff);
    sie::intr_on();

    asm!("
        li t0, -1
        csrw pmpaddr0, t0
        li t0, 0x7f
        csrw pmpcfg0, t0
    ");

    // 请求时钟中断
    timerinit();

    // 将每个 CPU 的 hartid 保持在其 tp 寄存器中，以供 cpuid () 使用。
    let id = mhartid::read();
    tp::write(id);

    // 切换到监管模式并跳转到 main () 函数。
    asm!("mret");

    // 不能在这里panic 或 print
    loop {}
}

/// 初始化机器模式定时器中断
///
/// # 功能说明
/// 配置每个CPU核心的定时器中断，使其定期触发。
/// 中断将由`timervec`处理（在汇编中定义），
/// 该处理程序将机器模式中断转换为监督者模式的软件中断。
///
/// # 流程解释
/// 1. 获取当前核心ID(hartid)
/// 2. 通过CLINT设置MTIMECMP寄存器（触发中断的时间点）
/// 3. 在MSCRATCH0中准备定时器中断处理所需信息
/// 4. 设置mscratch寄存器指向当前核心的上下文存储区
/// 5. 设置机器模式陷阱处理程序为timervec
/// 6. 启用机器模式中断(MIE)和定时器中断(MTIE)
///
/// # 参数
/// 无
///
/// # 返回值
/// 无
///
/// # 安全性
/// - 访问全局数组MSCRATCH0需unsafe
/// - 直接操作硬件中断寄存器
unsafe fn timerinit() {
    // 每个 CPU 都有一个独立的定时器中断源。
    let id = mhartid::read();

    // 向 CLINT 请求一个定时器中断。
    let interval: u64 = 1000000; // 时钟周期；在 qemu 中大约0.1秒。
    clint::add_mtimecmp(id, interval);

    // 为 timervec 在 scratch [] 中准备信息。
    // scratch [0..3]：供 timervec 保存寄存器的空间。
    // scratch [4]：CLINT 的 MTIMECMP 寄存器的地址。
    // scratch [5]：定时器中断之间的期望间隔（以时钟周期为单位）。
    let offset = 32 * id;
    MSCRATCH0[offset + 4] = 8 * id + Into::<usize>::into(CLINT_MTIMECMP);
    MSCRATCH0[offset + 5] = interval as usize;
    mscratch::write((MSCRATCH0.as_ptr() as usize) + offset * core::mem::size_of::<usize>());

    // 设置机器模式的陷阱处理程序。
    extern "C" {
        fn timervec();
    }
    mtvec::write(timervec as usize);

    // 启用机器模式中断。
    mstatus::set_mie();

    // 启用机器模式定时器中断。
    mie::set_mtie();
}
