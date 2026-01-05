//! 进程状态管理，包含fork，sleep等多种进程状态操作

use array_macro::array;

use alloc::boxed::Box;
use alloc::sync::Arc;
use core::mem;
use core::sync::atomic::{AtomicBool, Ordering};
use core::option::Option;
use core::ptr;
use core::cell::UnsafeCell;

use crate::consts::{PGSIZE, fs::{NFILE, ROOTIPATH}};
use crate::mm::{PageTable, RawPage, RawSinglePage};
use crate::register::{satp, sepc, sstatus};
use crate::spinlock::{SpinLock, SpinLockGuard};
use crate::trap::user_trap;
use crate::fs::{Inode, ICACHE, LOG, File};

use super::CpuManager;
use super::PROC_MANAGER;
use super::cpu::CPU_MANAGER;
use super::{fork_ret, Context, TrapFrame};

use self::syscall::Syscall;

mod syscall;
mod elf;

/// 进程状态枚举类型，表示操作系统内核中进程的不同生命周期状态。
///
/// 该枚举用于进程调度与管理，反映进程当前的执行或等待状态，
/// 便于操作系统根据状态做出调度决策与资源回收处理。
#[derive(Clone, Copy, Eq, PartialEq, Debug)]
pub enum ProcState {
    /// 该进程槽位未被占用，空闲状态。
    UNUSED,
    /// 进程处于睡眠状态，等待某事件或资源唤醒。
    SLEEPING,
    /// 进程处于可运行状态，等待调度器调度执行。
    RUNNABLE,
    /// 进程当前正在 CPU 上运行。
    RUNNING,
    /// 进程已被分配但尚未准备好运行。
    ALLOCATED,
    /// 进程已退出，处于僵尸状态，等待父进程回收。
    ZOMBIE,
}


/// 进程的排他信息结构体，包含进程的核心状态和控制字段。
///
/// 该结构体保存进程的调度状态、退出码、等待通道及进程标识符等信息，
/// 通常由进程的排它锁保护，确保并发环境下的安全访问与修改。
pub struct ProcExcl {
    /// 进程当前的状态，类型为 [`ProcState`]，反映进程生命周期阶段。
    pub state: ProcState,
    /// 进程退出时的状态码，用于父进程获取子进程退出信息。
    pub exit_status: i32,
    /// 进程等待的通道标识，用于睡眠和唤醒机制的同步。
    pub channel: usize,
    /// 进程的唯一标识符（进程ID）。
    pub pid: usize,
}


impl ProcExcl {
    const fn new() -> Self {
        Self {
            state: ProcState::UNUSED,
            exit_status: 0,
            channel: 0,
            pid: 0,
        }
    }

    /// 清除 [`ProcExcl`]的内容
    pub fn cleanup(&mut self) {
        self.pid = 0;
        self.channel = 0;
        self.exit_status = 0;
        self.state = ProcState::UNUSED;
    }
}

/// 进程私有数据结构，保存进程运行时的核心信息。
///
/// 该结构体仅在当前进程运行时访问，或在持有 [`ProcExcl`] 锁的其他进程（例如 fork）
/// 初始化时访问。包含内核栈指针、内存大小、上下文、打开的文件、用户页表等私有资源。
pub struct ProcData {
    /// 进程内核栈的起始虚拟地址。
    kstack: usize,
    /// 进程使用的内存大小（字节数）。
    sz: usize,
    /// 进程上下文（寄存器状态等），用于上下文切换。
    context: Context,
    /// 进程名称，最长16字节，通常用于调试和显示。
    name: [u8; 16],
    /// 进程打开的文件数组，元素为可选的引用计数智能指针。
    open_files: [Option<Arc<File>>; NFILE],
    /// 指向 TrapFrame 的裸指针，保存用户态寄存器临时值等信息。
    pub tf: *mut TrapFrame,
    /// 进程的用户页表，管理用户地址空间映射。
    pub pagetable: Option<Box<PageTable>>,
    /// 进程当前工作目录的 inode。
    pub cwd: Option<Inode>,
}


impl ProcData {
    const fn new() -> Self {
        Self {
            kstack: 0,
            sz: 0,
            context: Context::new(),
            name: [0; 16],
            open_files: array![_ => None; NFILE],
            tf: ptr::null_mut(),
            pagetable: None,
            cwd: None,
        }
    }

    /// Set kstack
    pub fn set_kstack(&mut self, kstack: usize) {
        self.kstack = kstack;
    }

    /// # 功能说明
    /// 初始化进程的上下文信息。该函数在进程创建后调用，
    /// 将进程上下文清零，并设置返回地址为 `fork_ret`，
    /// 以便进程切换到用户态时从 `fork_ret` 函数开始执行。
    ///
    /// # 流程解释
    /// 1. 调用 `context.clear()` 清空当前上下文寄存器状态。
    /// 2. 设置上下文的返回地址寄存器（ra）为 `fork_ret` 函数的地址，
    ///    确保进程切换后执行 fork 返回逻辑。
    /// 3. 设置栈指针（sp）指向内核栈顶（`kstack + PGSIZE*4`），
    ///    以保证内核栈空间正确。
    ///
    /// # 参数
    /// - `&mut self`：当前进程的可变引用，用于修改其上下文和内核栈指针。
    ///
    /// # 返回值
    /// - 无返回值。
    pub fn init_context(&mut self) {
        self.context.clear();
        self.context.set_ra(fork_ret as *const () as usize);
        self.context.set_sp(self.kstack + PGSIZE*4);
    }

    /// Return the process's mutable reference of context
    pub fn get_context(&mut self) -> *mut Context {
        &mut self.context as *mut _
    }

    /// # 功能说明
    /// 准备进程从内核态返回到用户态所需的 TrapFrame 和寄存器状态，
    /// 并返回用户页表的 satp 寄存器值以切换地址空间。
    ///
    /// # 流程解释
    /// 1. 获取当前进程的 TrapFrame，可修改其中的内核态相关字段。
    /// 2. 读取当前内核页表的 satp 寄存器值，保存到 `tf.kernel_satp`，
    ///    用于内核态返回时恢复内核页表映射。
    /// 3. 设置内核栈指针 `tf.kernel_sp` 指向内核栈顶（`kstack + PGSIZE*4`）。
    /// 4. 设置内核陷阱处理入口 `tf.kernel_trap` 为用户态陷阱处理函数地址 `user_trap`。
    /// 5. 设置当前 CPU 核心编号到 `tf.kernel_hartid`。
    /// 6. 将之前保存在 TrapFrame 的用户程序计数器 `epc` 写回 sepc 寄存器，
    ///    用于从陷阱返回后继续执行用户程序。
    /// 7. 返回当前进程的用户页表的 satp 寄存器值，供汇编代码切换页表。
    ///
    /// # 参数
    /// - `&mut self`：当前进程的可变引用，用于修改其 TrapFrame 和页表信息。
    ///
    /// # 返回值
    /// - 返回 `usize` 类型的用户页表的 satp 寄存器值，用于地址空间切换。
    ///
    /// # 可能的错误
    /// - 函数中使用了多处 `unwrap()` 和 `unsafe`，
    ///   若 `tf` 或 `pagetable` 未正确初始化，可能触发 panic 或未定义行为。
    /// - 需保证当前线程确实持有对进程数据的独占访问。
    ///
    /// # 安全性
    /// - 本函数包含 `unsafe` 代码块，假设 `tf` 指针有效且进程页表已正确初始化。
    /// - 调用者需确保在进程调度上下文中调用此函数，避免数据竞争。
    /// - 返回的 satp 值需用于低级上下文切换汇编代码，确保切换正确执行。
    pub fn user_ret_prepare(&mut self) -> usize {
        let tf: &mut TrapFrame = unsafe { self.tf.as_mut().unwrap() };
        tf.kernel_satp = satp::read();
        // 当前内核栈的内容已被清理
        // 返回内核空间之后
        tf.kernel_sp = self.kstack + PGSIZE*4;
        tf.kernel_trap = user_trap as usize;
        tf.kernel_hartid = unsafe { CpuManager::cpu_id() };

        // 恢复之前存储在 sepc 中的用户程序计数器
        sepc::write(tf.epc);

        self.pagetable.as_ref().unwrap().as_satp()
    }

    /// 简单检查用户传入的虚拟地址是否在合法范围内。
    fn check_user_addr(&self, user_addr: usize) -> Result<(), ()> {
        if user_addr > self.sz {
            Err(())
        } else {
            Ok(())
        }
    }

    /// 将内容从 src 复制到用户的目标虚拟地址 dst。
    /// 总共复制 count 字节。
    /// 实际操作会转发调用到页表的对应方法。
    #[inline]
    pub fn copy_out(&mut self, src: *const u8, dst: usize, count: usize) -> Result<(), ()> {
        self.pagetable.as_mut().unwrap().copy_out(src, dst, count)
    }

    /// 将内容从用户的源虚拟地址 src 复制到内核空间的目标地址 dst。
    /// 总共复制 count 字节。
    /// 实际操作会转发调用到页表的对应方法。
    #[inline]
    pub fn copy_in(&self, src: usize, dst: *mut u8, count: usize) -> Result<(), ()> {
        self.pagetable.as_ref().unwrap().copy_in(src, dst, count)
    }

    /// 分配一个新的文件描述符。
    /// 返回的文件描述符可直接作为索引使用，因为它仅属于当前进程私有。
    fn alloc_fd(&mut self) -> Option<usize> {
        self.open_files.iter()
            .enumerate()
            .find(|(_, f)| f.is_none())
            .map(|(i, _)| i)
    }

    /// 分配一对文件描述符。
    /// 通常用于管道（pipe）的创建。
    fn alloc_fd2(&mut self) -> Option<(usize, usize)> {
        let mut iter = self.open_files.iter()
            .enumerate()
            .filter(|(_, f)| f.is_none())
            .take(2)
            .map(|(i, _)| i);
        let fd1 = iter.next()?;
        let fd2 = iter.next()?;
        Some((fd1, fd2))
    }

    /// # 功能说明
    /// 清理进程私有数据中的部分资源状态，主要用于进程退出或重用时的复位。
    /// 该函数会清空进程名称首字节，释放与进程相关的 trapframe 内存，
    /// 并释放进程的用户页表对应的内存区域，同时重置进程的内存大小。
    ///
    /// # 流程解释
    /// 1. 将进程名称数组的第一个字节置为 0，标记名称为空。
    /// 2. 保存当前的 `tf`（TrapFrame）裸指针，并将结构体中的 `tf` 指针置空。
    /// 3. 若原 `tf` 指针非空，调用不安全代码通过 `RawSinglePage::from_raw_and_drop` 释放该内存。
    /// 4. 取出并移除当前进程的用户页表（`pagetable`）。
    /// 5. 若页表存在，调用 `dealloc_proc_pagetable` 释放进程占用的用户内存页面。
    /// 6. 重置进程占用的内存大小 `sz` 为 0。
    ///
    /// # 参数
    /// - `&mut self`：当前进程私有数据的可变引用，用于修改和释放其资源。
    ///
    /// # 返回值
    /// - 无返回值。
    ///
    /// # 可能的错误
    /// - 如果 `tf` 指针指向的内存已经被其他代码释放，调用 `from_raw_and_drop` 可能导致未定义行为。
    /// - 若 `pagetable` 未正确初始化，调用 `dealloc_proc_pagetable` 可能引发错误或崩溃。
    ///
    /// # 安全性
    /// - 该函数包含不安全代码，依赖于 `tf` 指针的有效性和唯一所有权。
    /// - 调用者必须确保在进程数据被其他代码访问之前调用此函数，避免资源竞争。
    /// - 释放页表时必须保证当前进程内存映射处于可安全释放状态，避免悬挂指针。
    pub fn cleanup(&mut self) {
        self.name[0] = 0;
        let tf = self.tf;
        self.tf = ptr::null_mut();
        if !tf.is_null() {
            unsafe { RawSinglePage::from_raw_and_drop(tf as *mut u8); }
        }
        let pgt = self.pagetable.take();
        if let Some(mut pgt) = pgt {
            pgt.dealloc_proc_pagetable(self.sz);
        }
        self.sz = 0;
    }

    /// # 功能说明
    /// 关闭进程打开的所有文件，并释放当前工作目录的引用。
    /// 该函数通常在进程退出时调用，用于清理进程的文件资源和目录引用。
    ///
    /// # 流程解释
    /// 1. 遍历进程打开的文件句柄数组 `open_files`，逐个取出并释放文件引用。
    /// 2. 调用日志系统 `LOG` 的 `begin_op()`，开始一次文件系统操作。
    /// 3. 使用断言确保当前工作目录 `cwd` 不为空。
    /// 4. 释放当前工作目录的引用（调用 `take()` 后立即 drop）。
    /// 5. 调用 `LOG.end_op()` 结束日志操作。
    ///
    /// # 参数
    /// - `&mut self`：当前进程私有数据的可变引用，用于操作其文件和目录成员。
    ///
    /// # 返回值
    /// - 无返回值。
    ///
    /// # 可能的错误
    /// - 若 `cwd` 为 `None`，`debug_assert!` 会在调试模式下触发断言失败。
    /// - 释放文件句柄和目录引用过程中，若底层文件系统操作失败，可能影响资源释放完整性（依赖日志系统机制）。
    ///
    /// # 安全性
    /// - 函数依赖外部日志系统 `LOG` 正确管理文件系统操作的事务一致性。
    /// - 关闭文件和释放目录引用必须确保调用时无其他线程或代码持有相关资源，避免竞态条件。
    /// - 本函数无不安全代码调用，符合 Rust 安全规范。
    pub fn close_files(&mut self) {
        for f in self.open_files.iter_mut() {
            drop(f.take())
        }
        LOG.begin_op();
        debug_assert!(self.cwd.is_some());
        drop(self.cwd.take());
        LOG.end_op();
    }

    /// # 功能说明
    /// 调整进程的用户堆大小，实现类似 UNIX 中的 `sbrk` 功能。
    /// 根据参数 `increment` 增加或减少用户地址空间的大小，
    /// 并相应地分配或释放物理内存页面。
    ///
    /// # 流程解释
    /// 1. 记录当前内存大小 `old_size` 以备返回。
    /// 2. 若 `increment` 大于 0，计算新的堆大小 `new_size`，
    ///    调用页表的 `uvm_alloc` 分配对应内存区域，更新进程内存大小。
    /// 3. 若 `increment` 小于 0，计算减少后的堆大小 `new_size`，
    ///    调用页表的 `uvm_dealloc` 释放对应内存区域，更新进程内存大小。
    /// 4. 返回调整前的内存大小 `old_size`。
    ///
    /// # 参数
    /// - `&mut self`：当前进程私有数据的可变引用，用于访问和修改内存大小及页表。
    /// - `increment`：调整的字节数，正数表示扩展堆空间，负数表示缩减堆空间。
    ///
    /// # 返回值
    /// - `Ok(usize)`：返回调整前的堆大小（字节数）。
    /// - `Err(())`：当内存分配失败时返回错误。
    ///
    /// # 可能的错误
    /// - 当调用 `uvm_alloc` 分配新内存失败时，返回 `Err(())`。
    /// - 负数缩减堆空间时未显式检查边界，可能出现内存越界或非法释放。
    ///
    /// # 安全性
    /// - 依赖 `pagetable` 正确初始化和有效性，`unwrap()` 可能引发 panic。
    /// - 调用者需保证调整操作在进程内存空间允许的范围内，避免非法访问。
    /// - 函数内部无使用不安全代码，符合 Rust 内存安全原则。
    fn sbrk(&mut self, increment: i32) -> Result<usize, ()> {
        let old_size = self.sz;
        if increment > 0 {
            let new_size = old_size + (increment as usize);
            self.pagetable.as_mut().unwrap().uvm_alloc(old_size, new_size)?;
            self.sz = new_size;
        } else if increment < 0 {
            let new_size = old_size - ((-increment) as usize);
            self.pagetable.as_mut().unwrap().uvm_dealloc(old_size, new_size);
            self.sz = new_size;
        }
        Ok(old_size)
    }
}

/// 进程结构体，代表操作系统内核中的一个进程实体。
///
/// 该结构体封装了进程在进程表中的索引，
/// 进程状态的排它锁保护数据（`ProcExcl`），
/// 进程私有数据（`ProcData`），
/// 以及进程是否被杀死的原子标志。
///
/// 通过该结构体，操作系统能够管理进程调度、状态更新和资源访问的并发安全。
pub struct Proc {
    /// 进程在进程表中的索引，唯一标识该进程槽位。
    index: usize,
    /// 进程排它锁保护的状态信息，包括状态、pid、等待通道等。
    pub excl: SpinLock<ProcExcl>,
    /// 进程私有数据，包含内存、上下文、文件描述符等，通过 UnsafeCell 实现内部可变性。
    pub data: UnsafeCell<ProcData>,
    /// 标识进程是否被杀死的原子布尔变量，用于调度和信号处理。
    pub killed: AtomicBool,
}

impl Proc {
    pub const fn new(index: usize) -> Self {
        Self {
            index,
            excl: SpinLock::new(ProcExcl::new(), "ProcExcl"),
            data: UnsafeCell::new(ProcData::new()),
            killed: AtomicBool::new(false),
        }
    }

    /// # 功能说明
    /// 初始化第一个用户进程的相关数据，包括加载初始化代码到用户页表、
    /// 设置用户程序计数器（PC）和栈指针（SP），
    /// 以及初始化进程名称和当前工作目录。
    ///
    /// # 流程解释
    /// 1. 获取当前进程的私有数据的可变引用 `pd`。
    /// 2. 使用 `uvm_init` 将内核预定义的初始化代码 `INITCODE` 映射到用户页表。
    /// 3. 设置进程内存大小 `sz` 为一页大小（`PGSIZE`）。
    /// 4. 获取进程的 TrapFrame 指针 `tf`，设置用户态程序计数器 `epc` 为 0，
    ///    栈指针 `sp` 为一页大小，准备用户态执行环境。
    /// 5. 将进程名称设置为 `"initcode"`，通过不安全的内存复制完成。
    /// 6. 断言当前工作目录 `cwd` 为空，确保进程尚未设置目录。
    /// 7. 通过根目录路径 `ROOTIPATH` 从 inode 缓存中获取根目录 inode，
    ///    并设置为当前工作目录。
    ///
    /// # 参数
    /// - `&mut self`：当前进程的可变引用，用于访问和修改其私有数据。
    ///
    /// # 返回值
    /// - 无返回值。
    ///
    /// # 可能的错误
    /// - 如果根目录 inode 无法找到，`expect` 会导致内核 panic，
    ///   表示文件系统初始化异常。
    /// - `unwrap` 调用若指针无效会触发 panic。
    ///
    /// # 安全性
    /// - 使用了不安全代码 `unsafe` 来操作裸指针和内存复制，
    ///   调用者需确保 `tf` 和 `name` 字段有效且可写。
    /// - 假定当前调用环境下独占访问 `ProcData`，避免数据竞争。
    pub fn user_init(&mut self) {
        let pd = self.data.get_mut();

        // 在用户页表中映射初始化代码
        pd.pagetable.as_mut().unwrap().uvm_init(&INITCODE);
        pd.sz = PGSIZE;

        // 准备返回程序计数器和栈指针
        let tf = unsafe { pd.tf.as_mut().unwrap() };
        tf.epc = 0;
        tf.sp = PGSIZE;

        let init_name = b"initcode\0";
        unsafe {
            ptr::copy_nonoverlapping(
                init_name.as_ptr(), 
                pd.name.as_mut_ptr(),
                init_name.len()
            );
        }

        debug_assert!(pd.cwd.is_none());
        pd.cwd = Some(ICACHE.namei(&ROOTIPATH).expect("cannot find root inode by b'/'"));
    }

    /// 如果 killed 标志为 true，则终止当前进程
    pub fn check_abondon(&mut self, exit_status: i32) {
        if self.killed.load(Ordering::Relaxed) {
            unsafe { PROC_MANAGER.exiting(self.index, exit_status); }
        }
    }

    /// 通过以下方式终止当前进程：
    /// 1. 将其 killed 标志设置为 true
    /// 2. 然后退出
    pub fn abondon(&mut self, exit_status: i32) {
        self.killed.store(true, Ordering::Relaxed);
        unsafe { PROC_MANAGER.exiting(self.index, exit_status); }
    }

    /// # 功能说明
    /// 处理当前进程发起的系统调用请求。根据 TrapFrame 中寄存器 a7 指定的系统调用号，
    /// 调用对应的系统调用处理函数，并将返回结果写回寄存器 a0。
    ///
    /// # 流程解释
    /// 1. 使能中断，允许系统中断处理。
    /// 2. 通过不安全代码获取当前进程的 TrapFrame 指针，读取系统调用号 `a7`。
    /// 3. 调用 `tf.admit_ecall()`，完成系统调用的相关状态处理（如跳过指令等）。
    /// 4. 使用 `match` 匹配系统调用号，调用对应的系统调用实现函数。
    /// 5. 若系统调用号非法，调用 `panic!` 抛出异常，终止内核执行。
    /// 6. 将系统调用执行结果写入 TrapFrame 的返回寄存器 `a0`，
    ///    成功返回实际结果，失败返回 -1（以 `usize` 格式存储）。
    ///
    /// # 参数
    /// - `&mut self`：当前进程的可变引用，用于访问其 TrapFrame 和调用系统调用实现。
    ///
    /// # 返回值
    /// - 无返回值，系统调用结果通过 TrapFrame 的 `a0` 寄存器返回给用户态。
    ///
    /// # 可能的错误
    /// - 系统调用号非法时，会导致内核 panic，内核崩溃或重启。
    /// - 各个系统调用具体实现可能返回错误，统一映射为返回值 -1。
    ///
    /// # 安全性
    /// - 使用了 `unsafe` 获取 TrapFrame 裸指针，假设指针有效且唯一所有权。
    /// - 该函数应在内核上下文且进程排他访问时调用，避免数据竞争。
    /// - 系统调用执行过程中可能包含更底层的 `unsafe`，调用此函数时需确保整体安全环境。
    pub fn syscall(&mut self) {
        sstatus::intr_on();

        let tf = unsafe { self.data.get_mut().tf.as_mut().unwrap() };
        let a7 = tf.a7;
        tf.admit_ecall();
        let sys_result = match a7 {
            1 => self.sys_fork(),
            2 => self.sys_exit(),
            3 => self.sys_wait(),
            4 => self.sys_pipe(),
            5 => self.sys_read(),
            6 => self.sys_kill(),
            7 => self.sys_exec(),
            8 => self.sys_fstat(),
            9 => self.sys_chdir(),
            10 => self.sys_dup(),
            11 => self.sys_getpid(),
            12 => self.sys_sbrk(),
            13 => self.sys_sleep(),
            14 => self.sys_uptime(),
            15 => self.sys_open(),
            16 => self.sys_write(),
            17 => self.sys_mknod(),
            18 => self.sys_unlink(),
            19 => self.sys_link(),
            20 => self.sys_mkdir(),
            21 => self.sys_close(),
            _ => {
                panic!("unknown syscall num: {}", a7);
            }
        };
        tf.a0 = match sys_result {
            Ok(ret) => ret,
            Err(()) => -1isize as usize,
        };
    }

    /// # 功能说明
    /// 让出当前进程的 CPU 使用权，将进程状态从运行中（RUNNING）
    /// 改为可运行（RUNNABLE），并调用调度器进行上下文切换，
    /// 以便其他进程获得执行机会。
    ///
    /// # 流程解释
    /// 1. 获取进程的排它锁 `excl`，保证状态修改的线程安全。
    /// 2. 断言当前进程状态为 `RUNNING`，确保进程处于运行态。
    /// 3. 将进程状态设置为 `RUNNABLE`，表示可被调度。
    /// 4. 调用当前 CPU 的调度函数 `sched`，传入当前进程的锁保护和上下文，
    ///    进行上下文切换，切换到其他进程执行。
    /// 5. 释放锁保护 `guard`。
    ///
    /// # 参数
    /// - `&mut self`：当前进程的可变引用，用于访问和修改进程状态及上下文。
    ///
    /// # 返回值
    /// - 无返回值，完成调度切换。
    ///
    /// # 可能的错误
    /// - 若进程当前状态不是 `RUNNING`，断言失败会导致内核 panic。
    /// - 调用 `sched` 函数过程中可能出现不可预期的调度错误。
    ///
    /// # 安全性
    /// - 使用了 `unsafe` 来获取 CPU 当前核心的可变引用，
    ///   假设调度器和 CPU 管理器状态正确且无竞争。
    /// - 调用此函数时应确保进程处于正确调度上下文，避免竞态条件。
    /// - 进程状态和上下文的修改均在锁保护下进行，保证线程安全。
    pub fn yielding(&mut self) {
        let mut guard = self.excl.lock();
        assert_eq!(guard.state, ProcState::RUNNING);
        guard.state = ProcState::RUNNABLE;
        guard = unsafe { CPU_MANAGER.my_cpu_mut().sched(guard,
            self.data.get_mut().get_context()) };
        drop(guard);
    }

    /// # 功能说明
    /// 原子地释放传入的自旋锁（非进程自身的锁），使当前进程进入睡眠状态，
    /// 并挂起在指定的等待通道 `channel` 上，等待被唤醒。
    /// 该函数不会在被唤醒后重新获取传入的锁，
    /// 需要调用者在必要时自行重新获取锁。
    ///
    /// # 流程解释
    /// 1. 获取进程自身的排它锁 `excl`，确保状态修改的安全性。
    /// 2. 释放传入的外部锁 `guard`，避免死锁（因为进程锁必须先获取）。
    /// 3. 设置进程等待通道为 `channel`，并将状态修改为 `SLEEPING`。
    /// 4. 调用当前 CPU 的调度器 `sched`，让出 CPU 并切换上下文，进入睡眠。
    /// 5. 睡眠被唤醒后，清空等待通道，释放进程锁。
    ///
    /// # 参数
    /// - `&self`：进程的不可变引用，用于访问排它锁和上下文。
    /// - `channel`：进程挂起等待的通道标识，用于唤醒匹配。
    /// - `guard`：传入的自旋锁保护的锁 guard，必须不是进程的排它锁，
    ///   用于在进入睡眠前释放，避免死锁。
    ///
    /// # 返回值
    /// - 无返回值，完成睡眠操作。
    ///
    /// # 可能的错误
    /// - 传入的 `guard` 若为进程自身的排它锁，会导致死锁。
    /// - 调用 `sched` 过程中若发生上下文切换异常可能导致系统调度异常。
    ///
    /// # 安全性
    /// - 使用 `unsafe` 代码获取并操作 CPU 相关资源和进程上下文，
    ///   需要保证指针有效且调用环境正确。
    /// - 保证在调用时持有适当的锁，避免竞态条件和死锁。
    /// - 进程状态和通道的修改均在锁保护下完成，保证线程安全。
    pub fn sleep<T>(&self, channel: usize, guard: SpinLockGuard<'_, T>) {
        // 必须先获取 p->lock 锁，才能修改 p->state，然后调用 sched。
        // 一旦我们持有 p->lock 锁，就可以确保不会错过任何唤醒操作（唤醒操作会锁定 p->lock），因此释放 lk 锁是安全的。
        let mut excl_guard = self.excl.lock();
        drop(guard);

        // 进入睡眠
        excl_guard.channel = channel;
        excl_guard.state = ProcState::SLEEPING;

        unsafe {
            let c = CPU_MANAGER.my_cpu_mut();
            excl_guard = c.sched(excl_guard, 
                &mut (*self.data.get()).context as *mut _);
        }

        excl_guard.channel = 0;
        drop(excl_guard);
    }

    /// # 功能说明
    /// 创建当前进程的一个子进程（fork），
    /// 复制父进程的内存、TrapFrame、打开文件、当前工作目录等信息，
    /// 并将子进程状态设置为可运行。
    ///
    /// # 流程解释
    /// 1. 获取当前进程的私有数据引用 `pdata`。
    /// 2. 通过 `PROC_MANAGER.alloc_proc()` 分配一个新的子进程，
    ///    若失败则返回错误 `Err(())`。
    /// 3. 获取子进程的排它锁 `cexcl` 和私有数据 `cdata`。
    /// 4. 复制父进程的用户内存到子进程页表，调用 `uvm_copy`。
    ///    若复制失败，清理子进程相关资源，返回错误。
    /// 5. 设置子进程的内存大小 `sz` 与父进程一致。
    /// 6. 复制 TrapFrame（用户寄存器状态），并将子进程的返回值寄存器 `a0` 设为 0。
    /// 7. 克隆父进程的打开文件数组和当前工作目录。
    /// 8. 复制父进程名称到子进程。
    /// 9. 记录子进程的进程 ID（pid）。
    /// 10. 设置子进程的父进程为当前进程。
    /// 11. 将子进程状态置为 `RUNNABLE`，表示可调度。
    /// 12. 返回子进程的进程 ID。
    ///
    /// # 参数
    /// - `&mut self`：当前进程的可变引用，用于访问自身私有数据和状态。
    ///
    /// # 返回值
    /// - `Ok(usize)`：子进程的进程 ID（pid）。
    /// - `Err(())`：分配子进程或复制内存失败时返回错误。
    ///
    /// # 可能的错误
    /// - 子进程分配失败（如进程表满），返回 `Err(())`。
    /// - 复制父进程内存失败时，清理子进程并返回错误。
    /// - 若 TrapFrame 指针无效，`unsafe` 操作可能导致未定义行为。
    ///
    /// # 安全性
    /// - 使用了多处 `unsafe` 操作，包括裸指针复制和子进程数据访问，
    ///   假设指针有效且内存分配正确。
    /// - 调用者需保证进程状态和私有数据在调用时无并发冲突。
    /// - 子进程资源清理确保不产生内存泄漏和悬挂指针。
    fn fork(&mut self) -> Result<usize, ()> {
        let pdata = self.data.get_mut();
        let child = unsafe { PROC_MANAGER.alloc_proc().ok_or(())? };
        let mut cexcl = child.excl.lock();
        let cdata = unsafe { child.data.get().as_mut().unwrap() };

        // 克隆内存
        let cpgt = cdata.pagetable.as_mut().unwrap();
        let size = pdata.sz;
        if pdata.pagetable.as_mut().unwrap().uvm_copy(cpgt, size).is_err() {
            debug_assert_eq!(child.killed.load(Ordering::Relaxed), false);
            child.killed.store(false, Ordering::Relaxed);
            cdata.cleanup();
            cexcl.cleanup();
            return Err(())
        }
        cdata.sz = size;

        // 克隆陷阱帧并在 a0 寄存器上返回 0
        unsafe {
            ptr::copy_nonoverlapping(pdata.tf, cdata.tf, 1);
            cdata.tf.as_mut().unwrap().a0 = 0;
        }

        // 克隆已打开的文件和当前工作目录
        cdata.open_files.clone_from(&pdata.open_files);
        cdata.cwd.clone_from(&pdata.cwd);
        
        // 复制进程名称
        cdata.name.copy_from_slice(&pdata.name);

        let cpid = cexcl.pid;

        drop(cexcl);

        unsafe { PROC_MANAGER.set_parent(child.index, self.index); }

        let mut cexcl = child.excl.lock();
        cexcl.state = ProcState::RUNNABLE;
        drop(cexcl);

        Ok(cpid)
    }
}

impl Proc {
    /// # 功能说明
    /// 从当前进程的 TrapFrame 中获取第 `n` 个系统调用参数的原始值（usize 类型）。
    /// 系统调用参数通过寄存器 a0~a5 传递，`n` 指定参数索引（0 到 5）。
    ///
    /// # 流程解释
    /// 1. 通过不安全代码获取当前进程的 TrapFrame 引用，确保指针有效。
    /// 2. 使用 match 匹配参数索引 `n`，返回对应寄存器 a0~a5 的值。
    /// 3. 若 `n` 大于 5，调用 panic 抛出异常，表明参数索引超出范围。
    ///
    /// # 参数
    /// - `&self`：当前进程不可变引用，用于访问 TrapFrame。
    /// - `n`：参数索引，范围为 0 至 5。
    ///
    /// # 返回值
    /// - 返回指定参数的原始寄存器值，类型为 usize。
    fn arg_raw(&self, n: usize) -> usize {
        let tf = unsafe { self.data.get().as_ref().unwrap().tf.as_ref().unwrap() };
        match n {
            0 => {tf.a0}
            1 => {tf.a1}
            2 => {tf.a2}
            3 => {tf.a3}
            4 => {tf.a4}
            5 => {tf.a5}
            _ => { panic!("n is larger than 5") }
        }
    }

    /// 获取 32 位寄存器的值。
    /// 注意：在 usize 和 i32 之间会进行as转换
    #[inline]
    fn arg_i32(&self, n: usize) -> i32 {
        self.arg_raw(n) as i32
    }

    /// 从寄存器值中获取原始用户虚拟地址。
    /// 注意：此原始地址可能为 null，
    /// 且它可能仅用于访问用户虚拟地
    #[inline]
    fn arg_addr(&self, n: usize) -> usize {
        self.arg_raw(n)
    }

    /// # 功能说明
    /// 从指定的系统调用参数寄存器中获取文件描述符（fd）
    /// 并检查该文件描述符是否合法且已被打开。
    ///
    /// # 流程解释
    /// 1. 调用 `arg_raw` 获取第 `n` 个参数的原始值，视为文件描述符。
    /// 2. 检查文件描述符是否超出最大允许值 `NFILE`。
    /// 3. 检查该文件描述符对应的文件是否存在（是否为 `Some`）。
    /// 4. 若检查通过，返回文件描述符；否则返回错误。
    ///
    /// # 参数
    /// - `&mut self`：当前进程可变引用，用于访问打开的文件数组。
    /// - `n`：参数索引，指明从第几个寄存器读取文件描述符。
    ///
    /// # 返回值
    /// - `Ok(usize)`：合法且打开的文件描述符。
    /// - `Err(())`：无效或未打开的文件描述符。
    ///
    /// # 可能的错误
    /// - 文件描述符超过允许的最大值 `NFILE`。
    /// - 文件描述符对应的文件句柄为 `None`，表示文件未打开。
    ///
    /// # 安全性
    /// - 该函数内部调用 `arg_raw` 使用了 `unsafe`，需保证寄存器指针有效。
    /// - 读取和判断文件句柄时，确保没有并发修改导致状态不一致。
    #[inline]
    fn arg_fd(&mut self, n: usize) -> Result<usize, ()> {
        let fd = self.arg_raw(n);
        if fd >= NFILE || self.data.get_mut().open_files[fd].is_none() {
            Err(())
        } else {
            Ok(fd)
        }
    }

    /// # 功能说明
    /// 从系统调用参数寄存器中获取一个指向用户空间的字符串指针，
    /// 将该以 null 结尾的字符串复制到内核缓冲区 `buf` 中。
    ///
    /// # 流程解释
    /// 1. 调用 `arg_raw` 获取第 `n` 个参数的原始值，视为用户虚拟地址字符串指针 `addr`。
    /// 2. 通过 `UnsafeCell` 获取当前进程的用户页表引用 `pagetable`。
    /// 3. 调用页表的 `copy_in_str` 方法，从用户虚拟地址空间复制字符串到 `buf`。
    /// 4. 若复制成功，返回 `Ok(())`，否则返回错误。
    ///
    /// # 参数
    /// - `&self`：当前进程不可变引用，用于访问页表和寄存器。
    /// - `n`：参数索引，指定从第几个寄存器读取字符串指针。
    /// - `buf`：用于存放复制进来的字符串的内核缓冲区。
    ///
    /// # 返回值
    /// - `Ok(())`：字符串复制成功。
    /// - `Err(&'static str)`：复制失败，可能是地址非法或未映射。
    ///
    /// # 可能的错误
    /// - 用户传入的指针非法，超出进程地址空间范围。
    /// - 字符串未正确以 null 结尾导致复制失败。
    /// - 页表查找或映射异常。
    ///
    /// # 安全性
    /// - 使用了 `unsafe` 访问裸指针，假设页表和数据有效。
    /// - 复制操作仅读用户空间，不修改数据，安全性较高。
    /// - 需要保证缓冲区 `buf` 大小足够存放用户字符串。
    fn arg_str(&self, n: usize, buf: &mut [u8]) -> Result<(), &'static str> {
        let addr: usize = self.arg_raw(n);
        let pagetable = unsafe { self.data.get().as_ref().unwrap().pagetable.as_ref().unwrap() };
        pagetable.copy_in_str(addr, buf)?;
        Ok(())
    }

    /// # 功能说明
    /// 从用户虚拟地址 `addr` 处读取一个 `usize` 类型的数据。
    /// 用于获取用户空间中存储的地址或数值。
    ///
    /// # 流程解释
    /// 1. 通过不安全代码获取当前进程的私有数据引用 `pd`。
    /// 2. 检查请求读取的地址范围是否超出进程当前内存大小 `sz`。
    ///    如果越界，返回错误。
    /// 3. 在缓冲变量 `ret` 中为数据分配空间。
    /// 4. 调用 `copy_in` 将用户虚拟地址 `addr` 处的内容复制到内核缓冲 `ret`。
    /// 5. 根据复制结果返回成功的读取值或错误信息。
    ///
    /// # 参数
    /// - `&self`：当前进程不可变引用，用于访问其内存数据和页表。
    /// - `addr`：用户虚拟地址，指向待读取的 `usize` 数据。
    ///
    /// # 返回值
    /// - `Ok(usize)`：成功读取用户地址处的数据。
    /// - `Err(&'static str)`：失败，返回错误字符串描述。
    ///
    /// # 可能的错误
    /// - 读取地址超出进程内存大小，返回地址越界错误。
    /// - 用户页表的 `copy_in` 操作失败，返回拷贝错误。
    ///
    /// # 安全性
    /// - 依赖不安全代码访问进程私有数据指针，假设指针有效且唯一所有权。
    /// - 通过页表安全复制数据，避免直接裸指针访问用户空间，符合内核安全规范。
    /// - 调用者需保证地址合法且缓冲区足够存储数据。
    fn fetch_addr(&self, addr: usize) -> Result<usize, &'static str> {
        let pd = unsafe { self.data.get().as_ref().unwrap() };
        if addr + mem::size_of::<usize>() > pd.sz {
            Err("input addr > proc's mem size")
        } else {
            let mut ret: usize = 0;
            match pd.copy_in(
                addr, 
                &mut ret as *mut usize as *mut u8, 
                mem::size_of::<usize>()
            ) {
                Ok(_) => Ok(ret),
                Err(_) => Err("pagetable copy_in eror"),
            }
        }
    }

    ///从虚拟地址addr获取一个以空字符结尾的字符串到内核缓冲区中。
    fn fetch_str(&self, addr: usize, dst: &mut [u8]) -> Result<(), &'static str>{
        let pd = unsafe { self.data.get().as_ref().unwrap() };
        pd.pagetable.as_ref().unwrap().copy_in_str(addr, dst)
    }
}

/// 第一个调用 exec ("/init") 的用户程序
static INITCODE: [u8; 51] = [
    0x17, 0x05, 0x00, 0x00, 0x13, 0x05, 0x05, 0x02, 0x97, 0x05, 0x00, 0x00, 0x93, 0x85, 0x05, 0x02,
    0x9d, 0x48, 0x73, 0x00, 0x00, 0x00, 0x89, 0x48, 0x73, 0x00, 0x00, 0x00, 0xef, 0xf0, 0xbf, 0xff,
    0x2f, 0x69, 0x6e, 0x69, 0x74, 0x00, 0x00, 0x01, 0x20, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00,
];
