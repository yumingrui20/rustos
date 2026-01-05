//! 监督模式陷阱原因寄存器 (scause) 操作模块

const INTERRUPT: usize = 0x8000000000000000;
const INTERRUPT_SUPERVISOR_SOFTWARE: usize = INTERRUPT + 1;
const INTERRUPT_SUPERVISOR_EXTERNAL: usize = INTERRUPT + 9;
const EXCEPTION: usize = 0;
const EXCEPTION_ECALL_USER: usize = EXCEPTION + 8;

/// 陷阱原因类型枚举
///
/// 表示从 scause 解析出的主要陷阱类型，
/// 包含内核需要处理的关键事件。
pub enum ScauseType {
    Unknown,
    IntSSoft,
    IntSExt,
    ExcUEcall,
}

/// 读取 scause 寄存器的当前值
///
/// # 返回值
/// scause 寄存器的原始值 (usize)
///
/// # 示例
/// ```
/// let cause = scause::read();
/// println!("Trap cause: {:#x}", cause);
/// ```
#[inline]
pub fn read() -> usize {
    let ret: usize;
    unsafe {core::arch::asm!("csrr {}, scause", out(reg) ret);}
    ret
}

/// 解析并返回陷阱原因类型
///
/// # 功能说明
/// 将 scause 的原始值转换为 `ScauseType` 枚举，
/// 识别内核需要处理的关键陷阱类型。
///
/// # 返回值
/// 对应陷阱类型的枚举值
///
/// # 注意
/// 当前仅识别部分常见陷阱类型，其他类型返回 `Unknown`
///
/// # 示例
/// ```
/// match scause::get_scause() {
///     ScauseType::IntSSoft => println!("Software interrupt"),
///     ScauseType::ExcUEcall => println!("System call"),
///     _ => println!("Unknown trap"),
/// }
/// ```
pub fn get_scause() -> ScauseType {
    let scause = read();
    match scause {
        INTERRUPT_SUPERVISOR_SOFTWARE => ScauseType::IntSSoft,
        INTERRUPT_SUPERVISOR_EXTERNAL => ScauseType::IntSExt,
        EXCEPTION_ECALL_USER => ScauseType::ExcUEcall,
        _ => ScauseType::Unknown,
    }
}
