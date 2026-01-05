//! RISC-V PLIC（平台级中断控制器）驱动模块
//!
//! 负责管理外部设备中断，包括：
//! - 中断优先级配置
//! - 中断使能控制
//! - 中断声明与完成处理
//!
//! PLIC 是 RISC-V 系统中处理外部设备中断的核心组件，
//! 支持多级优先级和多个中断目标（HART）。
//!
//! # 主要功能
//! 1. 全局初始化 (`init`)
//! 2. 单核初始化 (`init_hart`)
//! 3. 中断声明 (`claim`)
//! 4. 中断完成 (`complete`)

use core::ptr;

use crate::process::CpuManager;
use crate::consts::{PLIC, UART0_IRQ, VIRTIO0_IRQ};

/// 初始化 PLIC 全局设置
///
/// # 功能说明
/// 设置关键设备中断的优先级（非零值启用中断）：
/// - UART0 (串口)：优先级 1
/// - VIRTIO0 (磁盘)：优先级 1
///
/// # 安全性
/// - 直接操作硬件寄存器
/// - 应在系统启动时调用一次
pub unsafe fn init() {
    // 设置UART中断优先级
    write(UART0_IRQ*4, 1);

    // 设置虚拟磁盘中断优先级
    write(VIRTIO0_IRQ*4, 1);
}

/// 初始化特定 CPU 核心的 PLIC 设置
///
/// # 参数
/// - `hart`: 目标 CPU 核心 ID
///
/// # 功能说明
/// 1. 启用当前核心的 UART 和 VIRTIO 中断
/// 2. 设置核心中断优先级阈值为 0（接收所有优先级中断）
///
/// # 安全性
/// - 直接操作硬件寄存器
/// - 应在每个核心启动时调用
pub unsafe fn init_hart(hart: usize) {
    // 启用当前核心的特定中断源
    write(SENABLE+SENABLE_HART*hart, (1<<UART0_IRQ)|(1<<VIRTIO0_IRQ));

    // 设置核心优先级阈值为0（接收所有中断）
    write(SPRIORITY+SPRIORITY_HART*hart, 0);
}

/// 声明当前待处理的中断
///
/// # 功能说明
/// 查询 PLIC 获取当前需要服务的中断号。
/// 该操作会获取最高优先级的中断并标记为处理中。
///
/// # 返回值
/// 中断号（0 表示无中断）
pub fn claim() -> u32 {
    let hart: usize = unsafe {CpuManager::cpu_id()};
    read(SCLAIM+SCLAIM_HART*hart)
}

/// 标记中断处理完成
///
/// # 参数
/// - `irq`: 已完成处理的中断号
///
/// # 功能说明
/// 通知 PLIC 指定中断已处理完成，
/// 允许再次触发该中断。
pub fn complete(irq: u32) {
    let hart: usize = unsafe {CpuManager::cpu_id()};
    write(SCLAIM+SCLAIM_HART*hart, irq);
}

// qemu 将可编程中断控制器放置在这里。
const PRIORITY: usize = 0x0;
const PENDING: usize = 0x1000;

const MENABLE: usize = 0x2000;
const MENABLE_HART: usize = 0x100;
const SENABLE: usize = 0x2080;
const SENABLE_HART: usize = 0x100;
const MPRIORITY: usize = 0x200000;
const MPRIORITY_HART: usize = 0x2000;
const SPRIORITY: usize = 0x201000;
const SPRIORITY_HART: usize = 0x2000;
const MCLAIM: usize = 0x200004;
const MCLAIM_HART: usize = 0x2000;
const SCLAIM: usize = 0x201004;
const SCLAIM_HART: usize = 0x2000;


/// 读取 PLIC 寄存器
///
/// # 参数
/// - `offset`: 寄存器偏移量
///
/// # 返回值
/// 寄存器当前值
///
/// # 安全性
/// - 使用 volatile 读取确保不被编译器优化
/// - 直接访问内存映射寄存器
#[inline]
fn read(offset: usize) -> u32 {
    unsafe {
        let src = (Into::<usize>::into(PLIC) + offset) as *const u32;
        ptr::read_volatile(src)
    }
}


/// 写入 PLIC 寄存器
///
/// # 参数
/// - `offset`: 寄存器偏移量
/// - `value`: 要写入的值
///
/// # 安全性
/// - 使用 volatile 写入确保不被编译器优化
/// - 直接访问内存映射寄存器
#[inline]
fn write(offset: usize, value: u32) {
    unsafe {
        let dst = (Into::<usize>::into(PLIC) + offset) as *mut u32;
        ptr::write_volatile(dst, value);
    }
}
