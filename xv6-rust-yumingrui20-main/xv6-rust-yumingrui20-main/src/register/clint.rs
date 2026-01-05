//! CLINT（核心本地中断器）操作模块
//!
//! 提供对 RISC-V 平台中 CLINT 组件的访问接口，主要用于处理定时器中断。
//! 参考文档：doc/FU540-C000-v1.0.pdf
//!
//! # 关键寄存器
//! - `mtime`: 64位全局计时器（所有核心共享）
//! - `mtimecmp`: 每个核心独立的64位计时器比较寄存器
//!
//! # 工作原理
//! 当 `mtime` 的值大于或等于某个核心的 `mtimecmp` 时，
//! 会触发该核心的定时器中断。通过定期更新 `mtimecmp`，
//! 可以实现周期性定时器中断。

use core::ptr;
use core::convert::Into;

use crate::consts::{CLINT_MTIME, CLINT_MTIMECMP};

/// 读取全局计时器值 (mtime)
///
/// # 功能说明
/// 获取自系统启动以来的时钟周期数。
///
/// # 返回值
/// 当前64位计时器值
///
/// # 安全性
/// - 直接访问内存映射寄存器
/// - 使用 volatile 读取确保不被编译器优化
#[inline]
unsafe fn read_mtime() -> u64 {
    ptr::read_volatile(Into::<usize>::into(CLINT_MTIME) as *const u64)
}

/// 写入核心的计时器比较寄存器 (mtimecmp)
///
/// # 参数
/// - `mhartid`: 目标核心ID
/// - `value`: 要设置的64位比较值
///
/// # 安全性
/// - 直接访问内存映射寄存器
/// - 需确保核心ID有效
#[inline]
unsafe fn write_mtimecmp(mhartid: usize, value: u64) {
    let offset = Into::<usize>::into(CLINT_MTIMECMP) + 8 * mhartid;
    ptr::write_volatile(offset as *mut u64, value);
}

/// 设置核心的下次定时器中断时间
///
/// # 功能说明
/// 基于当前时间设置未来的定时器中断：
///   新比较值 = 当前时间 + 间隔
///
/// # 参数
/// - `mhartid`: 目标核心ID
/// - `interval`: 中断间隔（时钟周期数）
///
/// # 安全性
/// - 直接访问硬件寄存器
/// - 需确保核心ID有效
///
/// # 示例
/// 设置核心0在1000000周期后触发中断：
/// ```
/// add_mtimecmp(0, 1000000);
/// ```
pub unsafe fn add_mtimecmp(mhartid: usize, interval: u64) {
    let value = read_mtime();
    write_mtimecmp(mhartid, value + interval);
}

/// 读取核心的计时器比较寄存器值
///
/// # 参数
/// - `mhartid`: 目标核心ID
///
/// # 返回值
/// 当前设置的64位比较值
///
/// # 安全性
/// - 直接访问内存映射寄存器
/// - 需确保核心ID有效
pub unsafe fn read_mtimecmp(mhartid: usize) -> u64 {
    let offset = Into::<usize>::into(CLINT_MTIMECMP) + 8 * mhartid;
    ptr::read_volatile(offset as *const u64)
}
