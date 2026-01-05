//! 机器状态寄存器 (mstatus) 操作模块

use core::arch::asm;

use bit_field::BitField;

/// 读取 mstatus 寄存器的当前值
///
/// # 返回值
/// mstatus 寄存器的当前值 (usize)
///
/// # 安全性
/// - 直接访问特权寄存器
#[inline]
unsafe fn read() -> usize {
    let ret: usize;
    asm!("csrr {}, mstatus", out(reg) ret);
    ret
}

/// 写入 mstatus 寄存器
///
/// # 参数
/// - `x`: 要设置的值
///
/// # 安全性
/// - 直接操作特权寄存器
/// - 需确保值符合预期位布局
#[inline]
unsafe fn write(x: usize) {
    asm!("csrw mstatus, {}", in(reg) x);
}

/// 机器模式前的特权模式 (MPP) 枚举
///
/// 表示执行 mret 指令后将返回的特权模式
pub enum MPP {
    User = 0,
    Supervisor = 1,
    Machine = 3,
}

/// 设置 MPP 字段 (机器模式前的特权模式)
///
/// # 功能说明
/// 配置执行 mret 指令后处理器将进入的特权模式。
///
/// # 参数
/// - `mpp`: 目标特权模式枚举值
///
/// # 安全性
/// - 修改特权寄存器状态
/// - 应在模式切换前设置
///
/// # 使用场景
/// 内核启动时设置返回监督模式：
/// ```
/// unsafe { mstatus::set_mpp(mstatus::MPP::Supervisor); }
/// ```
pub unsafe fn set_mpp(mpp: MPP) {
    let mut mstatus = read();
    mstatus.set_bits(11..13, mpp as usize);
    write(mstatus);
}

/// 使能机器模式全局中断 (MIE)
///
/// # 功能说明
/// 设置 mstatus 的第 3 位 (MIE)，允许机器模式处理中断。
/// 这是接收定时器中断和外部中断的必要条件。
///
/// # 安全性
/// - 修改特权寄存器状态
/// - 应在中断系统初始化后调用
///
/// # 注意
/// 需与 `mie::set_mtie()` 配合使用以启用特定中断
pub unsafe fn set_mie() {
    let mut mstatus = read();
    mstatus.set_bit(3, true);
    write(mstatus);
}
