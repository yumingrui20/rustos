use core::{sync::atomic::Ordering, num::Wrapping, ptr};
use core::convert::Into;

use crate::{consts::{UART0, driver::UART_BUF}, spinlock::SpinLock};
use crate::process::{CPU_MANAGER, PROC_MANAGER};
use crate::process::{push_off, pop_off};

use super::PANICKED;
use super::console;

/// 寄存器访问宏
///
/// # 功能说明
/// 将寄存器偏移量转换为物理地址
///
/// # 示例
/// `Reg!(LSR)` 返回 LSR 寄存器的物理地址
macro_rules! Reg {
    ($reg: expr) => {
        Into::<usize>::into(UART0) + $reg
    };
}

/// 寄存器读取宏
///
/// # 功能说明
/// 从指定寄存器读取一个字节（volatile 操作）
///
/// # 安全性
/// 需要有效的寄存器偏移量
macro_rules! ReadReg {
    ($reg: expr) => {
        unsafe { ptr::read_volatile(Reg!($reg) as *const u8) }
    };
}

/// 寄存器写入宏
///
/// # 功能说明
/// 向指定寄存器写入一个字节（volatile 操作）
///
/// # 安全性
/// 需要有效的寄存器偏移量
macro_rules! WriteReg {
    ($reg: expr, $value: expr) => {
        unsafe {
            ptr::write_volatile(Reg!($reg) as *mut u8, $value);
        }
    };
}

/// 初始化 UART 设备
///
/// # 功能说明
/// 配置 UART 设备参数：
/// 1. 禁用中断
/// 2. 设置波特率 (38.4K)
/// 3. 配置数据格式 (8N1)
/// 4. 启用 FIFO 缓冲区
/// 5. 启用接收中断
pub(super) fn init() {
    // 禁用中断
    WriteReg!(IER, 0x00);

    // 用于设置波特率的特殊模式
    WriteReg!(LCR, 0x80);

    // 38.4K 波特率的最低有效位
    WriteReg!(0, 0x03);

    // 38.4K 波特率的最高有效位
    WriteReg!(1, 0x00);

    // 退出设置波特率模式，
    // 并将字长设置为 8 位，无校验。
    WriteReg!(LCR, 0x03);

    //  重置并启用 FIFO
    WriteReg!(FCR, 0x07);

    // 启用接收中断
    WriteReg!(IER, 0x03);
}

/// 同步阻塞方式输出字符
///
/// # 功能说明
/// 1. 禁用中断 (push_off)
/// 2. 检查恐慌状态
/// 3. 等待 UART 空闲
/// 4. 发送字符
/// 5. 恢复中断状态 (pop_off)
///
/// # 参数
/// - `c`: 要发送的字符
///
/// # 注意
/// 在系统恐慌状态下会进入死循环
pub(super) fn putc_sync(c: u8) {
    push_off();
    if PANICKED.load(Ordering::Relaxed) {
        loop {}
    }
    while !is_idle() {}
    WriteReg!(THR, c);
    pop_off();
}

/// 全局 UART 实例（自旋锁保护）
pub static UART: SpinLock<Uart> = SpinLock::new(
    Uart {
        buf: [0; UART_BUF],
        ri: Wrapping(0),
        wi: Wrapping(0),
    },
    "uart",
);

/// 为自旋锁保护的 UART 实例添加扩展方法
impl SpinLock<Uart> {
    /// 异步输出字符到 UART
    ///
    /// # 功能说明
    /// 1. 检查恐慌状态
    /// 2. 如果缓冲区满，则阻塞当前进程
    /// 3. 将字符放入缓冲区
    /// 4. 尝试传输数据
    ///
    /// # 参数
    /// - `c`: 要发送的字符
    pub fn putc(&self, c: u8) {
        let mut uart = self.lock();

        if PANICKED.load(Ordering::Relaxed) {
            loop {}
        }

        loop {
            if uart.wi == uart.ri + Wrapping(UART_BUF) {
                let p = unsafe { CPU_MANAGER.my_proc() };
                p.sleep(&uart.ri as *const Wrapping<_> as usize, uart);
                uart = self.lock();
            } else {
                let wi = uart.wi.0 % UART_BUF;
                uart.buf[wi] = c;
                uart.wi += Wrapping(1);
                uart.transmit();
                break
            }
        }
    }

    /// UART 中断处理函数
    ///
    /// # 功能说明
    /// 1. 接收数据：读取所有可用字符并传递给控制台
    /// 2. 传输数据：尝试发送缓冲区中的字符
    pub fn intr(&self) {
        // receive
        loop {
            let c: u8;
            if ReadReg!(LSR) & 1 > 0 {
                c = ReadReg!(RHR);
            } else {
                break
            }
            console::intr(c);
        }

        // transmit
        self.lock().transmit();
    }
}

/// UART 内部状态结构
pub struct Uart {
    buf: [u8; UART_BUF],
    ri: Wrapping<usize>,
    wi: Wrapping<usize>,
}

impl Uart {
    /// 传输缓冲区内容
    ///
    /// # 功能说明
    /// 1. 当 UART 空闲且缓冲区有数据时
    /// 2. 从缓冲区读取字符
    /// 3. 发送到 THR 寄存器
    /// 4. 更新读索引
    /// 5. 唤醒可能等待的进程
    fn transmit(&mut self) {
        while self.wi != self.ri && is_idle() {
            let ri = self.ri.0 % UART_BUF;
            let c = self.buf[ri];
            self.ri += Wrapping(1);
            unsafe { PROC_MANAGER.wakeup(&self.ri as *const Wrapping<_> as usize); }
            WriteReg!(THR, c);
        }
    }
}

// 16550 UART 寄存器偏移量定义
// reference: http://byterunner.com/16550.html
const RHR: usize = 0;       // 接收保持寄存器 (读操作)
const THR: usize = 0;       // 传输保持寄存器 (写操作)
const IER: usize = 1;       // 中断使能寄存器
const FCR: usize = 2;       // FIFO 控制寄存器
const ISR: usize = 2;       // 中断状态寄存器 (只读)
const LCR: usize = 3;       // 线路控制寄存器
const LSR: usize = 5;       // 线路状态寄存器

/// 检查 UART 是否空闲（可发送数据）
///
/// # 返回值
/// - `true`: 传输保持寄存器空闲 (LSR[5] = 1)
/// - `false`: 传输保持寄存器忙
///
/// # 说明
/// LSR[5] 位表示传输保持寄存器是否为空
/// 当该位为 1 时，可以发送新数据
#[inline]
fn is_idle() -> bool {
    ReadReg!(LSR) & (1 << 5) > 0
}
