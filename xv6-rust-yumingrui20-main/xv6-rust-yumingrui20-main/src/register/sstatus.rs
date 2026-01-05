//! 监督状态寄存器 (sstatus) 操作模块

use core::arch::asm;

const SIE: usize = 1 << 1;  // supervisor interrupt enable
const SPIE: usize = 1 << 5; // supervisor previous interrupt enable
const SPP: usize = 1 << 8;  // previous mode, is from supervisor?

/// 读取 sstatus 寄存器的当前值
///
/// # 返回值
/// sstatus 寄存器的当前值 (usize)
///
/// # 示例
/// ```
/// let status = sstatus::read();
/// println!("Current sstatus: {:#x}", status);
/// ```
#[inline]
pub fn read() -> usize {
    let ret: usize;
    unsafe {asm!("csrr {}, sstatus", out(reg) ret);}
    ret
}

/// 写入 sstatus 寄存器
///
/// # 参数
/// - `x`: 要设置的值
///
/// # 注意事项
/// 直接修改整个寄存器可能影响多个状态位，
/// 建议使用特定功能函数进行部分修改
#[inline]
pub fn write(x: usize) {
    unsafe {asm!("csrw sstatus, {}", in(reg) x);}
}

/// 启用监督模式全局中断 (SIE)
///
/// # 功能说明
/// 设置 sstatus 的第 1 位 (SIE)，允许监督模式处理中断。
/// 这是接收软件中断、定时器中断和外部中断的必要条件。
///
/// # 注意事项
/// 1. 还需在 `sie` 寄存器中启用具体中断类型
/// 2. 中断需已委托到监督模式 (`mideleg`)
///
/// # 使用场景
/// 系统初始化完成后启用中断：
/// ```
/// sstatus::intr_on();
/// ```
#[inline]
pub fn intr_on() {
    write(read() | SIE);
}

/// 禁用监督模式全局中断 (SIE)
///
/// # 功能说明
/// 清除 sstatus 的第 1 位 (SIE)，暂时禁止监督模式处理中断。
/// 用于保护关键代码段不被中断打断。
///
/// # 使用场景
/// 进入关键代码段前：
/// ```
/// sstatus::intr_off();
/// // 执行关键操作...
/// sstatus::intr_on(); // 恢复中断
/// ```
#[inline]
pub fn intr_off() {
    write(read() & !SIE);
}

/// 检查全局中断是否已启用
///
/// # 返回值
/// - `true`: 中断已启用 (SIE = 1)
/// - `false`: 中断已禁用 (SIE = 0)
///
/// # 使用场景
/// 调试或状态检查：
/// ```
/// if sstatus::intr_get() {
///     println!("Interrupts are enabled");
/// }
/// ```
#[inline]
pub fn intr_get() -> bool {
    let x = read();
    (x & SIE) != 0
}

/// 检查陷阱是否来自监督模式
///
/// # 功能说明
/// 通过检查 SPP 位判断陷阱发生前的特权模式
///
/// # 返回值
/// - `true`: 陷阱发生前处于监督模式 (SPP = 1)
/// - `false`: 陷阱发生前处于用户模式 (SPP = 0) 或其他
///
/// # 使用场景
/// 陷阱处理中确定来源：
/// ```
/// if sstatus::is_from_supervisor() {
///     println!("Trap from supervisor mode");
/// }
/// ```
#[inline]
pub fn is_from_supervisor() -> bool {
    (read() & SPP) != 0
}

/// 检查陷阱是否来自用户模式
///
/// # 功能说明
/// 通过检查 SPP 位判断陷阱发生前的特权模式
///
/// # 返回值
/// - `true`: 陷阱发生前处于用户模式 (SPP = 0)
/// - `false`: 陷阱发生前处于监督模式 (SPP = 1)
///
/// # 使用场景
/// 系统调用验证：
/// ```
/// if sstatus::is_from_user() {
///     handle_syscall();
/// } else {
///     panic!("Syscall from supervisor mode!");
/// }
/// ```
#[inline]
pub fn is_from_user() -> bool {
    (read() & SPP) == 0
}

/// 准备返回用户空间
///
/// # 功能说明
/// 配置 sstatus 寄存器以安全返回用户空间：
/// 1. 清除 SPP 位 (设置为用户模式)
/// 2. 设置 SPIE 位 (保存当前中断使能状态)
/// 
/// # 流程解释
/// - 清除 SPP 位：确保 `sret` 后进入用户模式
/// - 设置 SPIE 位：保存当前中断使能状态
/// - 同时清除 SIE 位：在用户空间执行前禁用中断
///
/// # 使用场景
/// 从内核陷阱返回用户空间前：
/// ```
/// sstatus::user_ret_prepare();
/// // 设置用户页表等其他状态
/// asm!("sret"); // 返回用户空间
/// ```
///
/// # RISC-V 规范说明
/// 执行 `sret` 指令时会：
/// 1. 将 SPP 位恢复为特权模式 (0=用户, 1=监督)
/// 2. 将 SPIE 位复制到 SIE (恢复中断使能状态)
/// 3. 设置 SPIE = 1 (允许后续陷阱恢复中断)
#[inline]
pub fn user_ret_prepare() {
    let mut x = read();
    x &= !SPP;
    x |= SPIE;
    write(x);
}
