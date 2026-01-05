//! 定义操作所需RISC-V寄存器的接口

pub mod clint;
pub mod mie;
pub mod mstatus;
pub mod satp;
pub mod sie;
pub mod sip;
pub mod sstatus;
pub mod scause;

/// 机器异常委托寄存器 (medeleg) 操作
///
/// # 功能说明
/// 控制哪些异常从机器模式委托到监督模式处理
///
/// # 安全性
/// 直接操作特权寄存器，需在特权模式下使用
pub mod medeleg {
    /// 设置 medeleg 寄存器值
    ///
    /// # 参数
    /// - `medeleg`: 要设置的位掩码值
    ///
    /// # 示例
    /// 委托所有异常到监督模式：
    /// ```
    /// unsafe { medeleg::write(0xffff); }
    /// ```
    pub unsafe fn write(medeleg: usize) {
        core::arch::asm!("csrw medeleg, {}",in(reg)medeleg);
    }
}

/// 机器异常程序计数器 (mepc) 操作
///
/// # 功能说明
/// 存储异常返回地址，当执行 mret 时从此地址恢复执行
pub mod mepc {
    /// 设置 mepc 寄存器值
    ///
    /// # 参数
    /// - `mepc`: 要设置的返回地址
    ///
    /// # 使用场景
    /// 初始化启动时设置内核入口点
    pub unsafe fn write(mepc: usize) {
        core::arch::asm!("csrw mepc, {}", in(reg)mepc);
    }
}

/// 硬件线程ID寄存器 (mhartid) 操作
///
/// # 功能说明
/// 提供当前核心的唯一标识符
pub mod mhartid {
    /// 读取当前核心的硬件线程ID
    ///
    /// # 返回值
    /// 当前核心的数字ID
    pub unsafe fn read() -> usize {
        let ret: usize;
        core::arch::asm!("csrr {}, mhartid",out(reg)ret);
        ret
    }
}

/// 机器中断委托寄存器 (mideleg) 操作
///
/// # 功能说明
/// 控制哪些中断从机器模式委托到监督模式处理
pub mod mideleg {
    /// 设置 mideleg 寄存器值
    ///
    /// # 参数
    /// - `mideleg`: 要设置的位掩码值
    ///
    /// # 示例
    /// 委托所有中断到监督模式：
    /// ```
    /// unsafe { mideleg::write(0xffff); }
    /// ```
    pub unsafe fn write(mideleg: usize) {
        core::arch::asm!("csrw mideleg, {}", in(reg)mideleg);
    }
}

/// 机器模式暂存寄存器 (mscratch) 操作
///
/// # 功能说明
/// 通用临时寄存器，常用于存储指针或临时值
pub mod mscratch {
    /// 设置 mscratch 寄存器值
    ///
    /// # 参数
    /// - `mscratch`: 要存储的值
    ///
    /// # 使用场景
    /// 陷阱处理时保存上下文指针
    pub unsafe fn write(mscratch: usize) {
        core::arch::asm!("csrw mscratch, {}",in(reg)mscratch);
    }
}

/// 机器陷阱向量基址寄存器 (mtvec) 操作
///
/// # 功能说明
/// 设置机器模式陷阱处理程序的入口地址
pub mod mtvec {
    /// 设置 mtvec 寄存器值
    ///
    /// # 参数
    /// - `mtvec`: 陷阱处理程序入口地址
    ///
    /// # 使用场景
    /// 初始化时设置机器模式中断处理程序
    pub unsafe fn write(mtvec: usize) {
        core::arch::asm!("csrw mtvec, {}",in(reg)mtvec);
    }
}

/// 线程指针寄存器 (tp) 操作
///
/// # 功能说明
/// 用于存储当前核心的处理器指针或核心ID
pub mod tp {
    /// 读取 tp 寄存器值
    ///
    /// # 返回值
    /// 当前存储的线程指针值
    pub unsafe fn read() -> usize {
        let ret: usize;
        core::arch::asm!("mv {}, tp",out(reg)ret);
        ret
    }

    /// 设置 tp 寄存器值
    ///
    /// # 参数
    /// - `tp`: 要设置的值
    ///
    /// # 使用场景
    /// 启动时存储核心ID供 `cpuid()` 使用
    pub unsafe fn write(tp: usize) {
        core::arch::asm!("mv tp, {}", in(reg)tp);
    }
}

/// 监督陷阱向量基址寄存器 (stvec) 操作
///
/// # 功能说明
/// 设置监督模式陷阱处理程序的入口地址
pub mod stvec {
    /// 设置 stvec 寄存器值
    ///
    /// # 参数
    /// - `stvec`: 陷阱处理程序入口地址
    ///
    /// # 使用场景
    /// 初始化时设置监督模式中断处理程序
    pub unsafe fn write(stvec: usize) {
        core::arch::asm!("csrw stvec, {}", in(reg)stvec);
    }
}

/// 监督异常程序计数器 (sepc) 操作
///
/// # 功能说明
/// 存储监督模式异常返回地址
pub mod sepc {
    /// 读取 sepc 寄存器值
    ///
    /// # 返回值
    /// 当前异常返回地址
    pub fn read() -> usize {
        let ret: usize;
        unsafe {core::arch::asm!("csrr {}, sepc", out(reg)ret);}
        ret
    }

    /// 设置 sepc 寄存器值
    ///
    /// # 参数
    /// - `sepc`: 要设置的返回地址
    ///
    /// # 使用场景
    /// 陷阱处理中调整返回地址
    pub fn write(sepc: usize) {
        unsafe {core::arch::asm!("csrw sepc, {}", in(reg)sepc);}
    }
}

/// 监督陷阱值寄存器 (stval) 操作
///
/// # 功能说明
/// 存储与陷阱相关的附加信息：
/// - 页面错误：访问的虚拟地址
/// - 非法指令：指令本身
/// - 其他：与陷阱相关的特定数据
pub mod stval {
    /// 读取 stval 寄存器值
    ///
    /// # 返回值
    /// 与当前陷阱相关的附加信息
    pub fn read() -> usize {
        let ret: usize;
        unsafe { core::arch::asm!("csrr {}, stval", out(reg)ret);}
        ret
    }
}
