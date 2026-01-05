//! Console driver for user input and output.

use core::num::Wrapping;
use core::sync::atomic::Ordering;

use crate::consts::driver::*;
use crate::spinlock::SpinLock;
use crate::mm::Address;
use crate::process::{CPU_MANAGER, PROC_MANAGER};

use super::uart;

/// 初始化控制台驱动
///
/// # 功能说明
/// 调用底层UART驱动进行初始化
///
/// # 安全性
/// - 必须仅在系统启动时调用一次
/// - 调用位置：`rmain.rs:rust_main`
pub unsafe fn init() {
    uart::init();
}

/// 从控制台读取数据
///
/// # 功能说明
/// 从控制台缓冲区读取最多`tot`字节到目标地址：
/// 1. 当缓冲区无数据时阻塞进程
/// 2. 处理特殊控制字符（EOF, 换行等）
/// 3. 支持用户空间和内核空间地址
///
/// # 参数
/// - `dst`: 目标地址（用户/内核空间）
/// - `tot`: 请求读取的字节数
///
/// # 返回值
/// - `Ok(n)`: 实际读取的字节数
/// - `Err(())`: 进程被终止时返回错误
///
/// # 处理流程
/// 1. 获取控制台锁
/// 2. 当缓冲区为空时阻塞进程
/// 3. 从环形缓冲区读取字符
/// 4. 处理特殊字符（EOF, 换行）
/// 5. 复制字符到目标地址
pub(super) fn read(mut dst: Address, tot: u32) -> Result<u32, ()> {
    let mut console = CONSOLE.lock();

    let mut left = tot;
    while left > 0 {
        // 如果控制台缓冲区中没有可用数据
        // 等待直到控制台设备写入一些数据
        while console.ri == console.wi {
            let p = unsafe { CPU_MANAGER.my_proc() };
            if p.killed.load(Ordering::Relaxed) {
                return Err(())
            }
            p.sleep(&console.ri as *const Wrapping<_> as usize, console);
            console = CONSOLE.lock();
        }

        // 读取
        let c = console.buf[console.ri.0 % CONSOLE_BUF];
        console.ri += Wrapping(1);

        // 遇到 EOF
        // 提前返回
        if c == CTRL_EOT {
            if left < tot {
                console.ri -= Wrapping(1);
            }
            break;
        }

        // 复制到用户 / 内核空间内存
        if dst.copy_out(&c as *const u8, 1).is_err() {
            break;
        }

        // 更新
        dst = dst.offset(1);
        left -= 1;

        // 遇到换行符
        if c == CTRL_LF {
            break;
        }
    }

    Ok(tot - left)
}

/// 向控制台写入数据
///
/// # 功能说明
/// 从源地址读取`tot`字节输出到控制台：
/// 1. 逐字节读取并输出
/// 2. 支持用户空间和内核空间地址
///
/// # 参数
/// - `src`: 源地址（用户/内核空间）
/// - `tot`: 请求写入的字节数
///
/// # 返回值
/// - `Ok(n)`: 实际写入的字节数
/// - 部分写入时返回已写入字节数
pub(super) fn write(mut src: Address, tot: u32) -> Result<u32, ()> {
    for i in 0..tot {
        let mut c = 0u8;
        if src.copy_in(&mut c as *mut u8, 1).is_err() {
            return Ok(i)
        }
        uart::UART.putc(c);
        src = src.offset(1);
    }
    Ok(tot)
}

/// 向控制台输出单个字符
///
/// # 功能说明
/// 处理特殊退格字符：
/// - 退格键(CTRL+H)：输出"空格+退格"实现擦除效果
/// - 其他字符：直接输出
///
/// # 参数
/// - `c`: 要输出的字符
pub(crate) fn putc(c: u8) {
    if c == CTRL_BS {
        uart::putc_sync(CTRL_BS);
        uart::putc_sync(b' ');
        uart::putc_sync(CTRL_BS);
    } else {
        uart::putc_sync(c);
    }
}

/// 控制台中断处理程序
///
/// # 功能说明
/// 处理UART接收到的字符：
/// 1. 特殊控制字符处理（进程列表、删除行等）
/// 2. 普通字符回显和缓冲区管理
/// 3. 唤醒等待输入的进程
///
/// # 处理流程
/// 1. 用户输入字符
/// 2. UART触发中断
/// 3. 控制台处理中断
/// 4. 回显输入或执行控制操作
///
/// # 参数
/// - `c`: 接收到的字符
pub(super) fn intr(c: u8) {
    let mut console = CONSOLE.lock();

    match c {
        CTRL_PRINT_PROCESS => {
            todo!("print process list to debug")
        },
        CTRL_BS_LINE => {
            while console.ei != console.wi &&
                console.buf[(console.ei-Wrapping(1)).0 % CONSOLE_BUF] != CTRL_LF
            {
                console.ei -= Wrapping(1);
                putc(CTRL_BS);
            }
        },
        CTRL_BS | CTRL_DEL => {
            if console.ei != console.wi {
                console.ei -= Wrapping(1);
                putc(CTRL_BS);
            }
        }
        _ => {
            // 回显
            if c != 0 && (console.ei - console.ri).0 < CONSOLE_BUF {
                let c = if c == CTRL_CR { CTRL_LF } else { c };
                putc(c);
                let ei = console.ei.0 % CONSOLE_BUF;
                console.buf[ei] = c;
                console.ei += Wrapping(1);
                if c == CTRL_LF || c == CTRL_EOT || (console.ei - console.ri).0 == CONSOLE_BUF {
                    console.wi = console.ei;
                    unsafe { PROC_MANAGER.wakeup(&console.ri as *const Wrapping<_> as usize); }
                }
            }
        },
    }
}

static CONSOLE: SpinLock<Console> = SpinLock::new(
    Console {
        buf: [0; CONSOLE_BUF],
        ri: Wrapping(0),
        wi: Wrapping(0),
        ei: Wrapping(0),
    },
    "console",
);

struct Console {
    buf: [u8; CONSOLE_BUF],
    // 读索引
    ri: Wrapping<usize>,
    // 写索引
    wi: Wrapping<usize>,
    // 编辑索引
    ei: Wrapping<usize>,
}
