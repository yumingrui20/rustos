//! 监督地址转换与保护寄存器 (satp) 操作模块

use core::arch::asm;

/// 读取 satp 寄存器的当前值
///
/// # 返回值
/// satp 寄存器的当前值 (usize)
///
/// # 示例
/// ```
/// let current_satp = satp::read();
/// println!("Current SATP: {:#x}", current_satp);
/// ```
#[inline]
pub fn read() -> usize {
    let ret;
    unsafe {
        asm!("csrr {}, satp", out(reg) ret);
    }
    ret
}

/// 写入 satp 寄存器
///
/// # 参数
/// - `satp`: 要设置的值
///
/// # 注意事项
/// 1. 写入后需要执行 `sfence.vma` 指令刷新 TLB
/// 2. 仅应在特权模式下调用
///
/// # 示例
/// 设置 Sv39 分页模式：
/// ```
/// let root_ppn = get_root_page_ppn(); // 获取根页表物理页号
/// let satp_value = (8 << 60) | (root_ppn >> 12);
/// satp::write(satp_value);
/// unsafe { asm!("sfence.vma"); } // 刷新 TLB
/// ```
#[inline]
pub fn write(satp: usize) {
    unsafe {
        asm!("csrw satp, {}", in(reg) satp);
    }
}
