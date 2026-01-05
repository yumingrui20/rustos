//! 机器模式中断使能寄存器 (mie) 操作模块

use core::arch::asm;

use bit_field::BitField;

/// 读取 mie 寄存器的当前值
///
/// # 返回值
/// mie 寄存器的当前值 (usize)
///
/// # 安全性
/// - 直接访问特权寄存器
#[inline]
unsafe fn read() -> usize {
    let ret: usize;
    asm!("csrr {}, mie", out(reg)ret);
    ret
}

/// 写入 mie 寄存器
///
/// # 参数
/// - `x`: 要设置的值
///
/// # 安全性
/// - 直接操作特权寄存器
/// - 需确保值符合预期位布局
#[inline]
unsafe fn write(x: usize) {
    asm!("csrw mie, {}",in(reg)x);
}

/// 使能机器模式定时器中断 (MTIE)
///
/// # 功能说明
/// 设置 mie 寄存器的第 7 位 (MTIE)，允许机器模式接收定时器中断。
/// 定时器中断通常用于实现时间片轮转调度和系统时钟。
///
/// # 安全性
/// - 修改特权寄存器状态
/// - 应在中断系统初始化后调用
///
/// # 示例
/// ```
/// unsafe { mie::set_mtie(); }
/// ```
pub unsafe fn set_mtie() {
    let mut mie = read();
    mie.set_bit(7, true);
    write(mie);
}
