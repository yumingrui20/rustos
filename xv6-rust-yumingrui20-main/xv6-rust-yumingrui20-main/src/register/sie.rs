//! 监督中断使能寄存器 (sie) 操作模块

use core::arch::asm;

const SSIE: usize = 1 << 1; // software
const STIE: usize = 1 << 5; // timer
const SEIE: usize = 1 << 9; // external

/// 读取 sie 寄存器的当前值
///
/// # 返回值
/// sie 寄存器的当前值 (usize)
///
/// # 安全性
/// - 直接访问特权寄存器
#[inline]
unsafe fn read() -> usize {
    let ret: usize;
    asm!("csrr {}, sie", out(reg) ret);
    ret
}

/// 写入 sie 寄存器
///
/// # 参数
/// - `x`: 要设置的值
///
/// # 安全性
/// - 直接操作特权寄存器
/// - 需确保值符合预期位布局
#[inline]
unsafe fn write(x: usize) {
    asm!("csrw sie, {}", in(reg) x);
}

/// 启用所有监督模式中断
///
/// # 功能说明
/// 设置 sie 寄存器中的 SSIE、STIE 和 SEIE 位，
/// 使能监督模式的软件中断、定时器中断和外部中断。
///
/// # 注意事项
/// 1. 此函数仅设置 sie 寄存器
/// 2. 还需设置 `sstatus::set_sie()` 全局使能中断
/// 3. 需确保中断已委托到监督模式 (`mideleg::write()`)
///
/// # 安全性
/// - 修改特权寄存器状态
///
/// # 使用场景
/// 系统初始化时启用监督模式中断：
/// ```
/// unsafe {
///     // 委托中断到监督模式
///     mideleg::write(0xffff);
///     
///     // 全局使能中断
///     sstatus::set_sie();
///     
///     // 启用具体中断类型
///     sie::intr_on();
/// }
/// ```
pub unsafe fn intr_on() {
    let mut sie = read();
    sie |= SSIE | STIE | SEIE;
    write(sie);
}
