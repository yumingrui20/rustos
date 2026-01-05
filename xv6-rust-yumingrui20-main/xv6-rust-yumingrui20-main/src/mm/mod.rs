//! 内存管理模块

use alloc::boxed::Box;
use core::{alloc::AllocError, ptr};

use crate::consts::PGSIZE;
use crate::process::CPU_MANAGER;

pub use addr::{Addr, PhysAddr, VirtAddr};
pub use kvm::{kvm_init, kvm_init_hart, kvm_map, kvm_pa};
pub use pagetable::{PageTable, PteFlag};
pub use kalloc::{KernelHeap, KERNEL_HEAP};

mod addr;
pub mod kalloc;
mod kvm;
mod pagetable;
mod list;

/// 定义物理页帧分配接口，用于分配页大小对齐的内存块。
///
/// # 功能说明
/// 该trait为不同类型的内存页提供统一的分配/释放接口，核心功能包括：
/// - 分配归零的物理页
/// - 分配未初始化的物理页
/// - 安全释放已分配的物理页
/// 实现通常使用`Box::new_zeroed()`和`Box::into_raw()`完成内存分配。
///
/// # 类型参数
/// - `Self`：实现trait的具体类型，需满足页大小对齐要求
pub trait RawPage: Sized {
    /// 分配一个已归零的物理页
    ///
    /// # 功能说明
    /// 分配单页大小的物理内存并将其内容初始化为零，返回该页起始地址的原始指针。
    ///
    /// # 返回值
    /// - `*mut u8`：分配的物理页起始地址
    ///
    /// # 安全性
    /// - 返回的指针必须通过`from_raw_and_drop`正确释放
    /// - 调用者需确保不重复释放同一指针
    unsafe fn new_zeroed() -> *mut u8 {
        let boxed_page = Box::<Self>::new_zeroed().assume_init();
        Box::into_raw(boxed_page) as *mut u8
    }

    /// 尝试分配一个已归零的物理页
    ///
    /// # 功能说明
    /// 功能同`new_zeroed`，但内存不足时返回错误而非panic
    ///
    /// # 返回值
    /// - `Ok(*mut u8)`：成功分配的物理页起始地址
    /// - `Err(AllocError)`：内存分配失败
    ///
    /// # 安全性
    /// 同`new_zeroed`
    unsafe fn try_new_zeroed() -> Result<*mut u8, AllocError> {
        let boxed_page = Box::<Self>::try_new_zeroed()?.assume_init();
        Ok(Box::into_raw(boxed_page) as *mut u8)
    }

    /// 尝试分配未初始化的物理页
    ///
    /// # 功能说明
    /// 分配单页大小的物理内存但不初始化内容，返回该页起始地址的原始指针
    ///
    /// # 返回值
    /// - `Ok(*mut u8)`：成功分配的物理页起始地址
    /// - `Err(AllocError)`：内存分配失败
    ///
    /// # 安全性
    /// - 调用者必须初始化内存内容后再使用
    /// - 其他安全要求同`new_zeroed`
    unsafe fn try_new_uninit() -> Result<*mut u8, AllocError> {
        let boxed_page = Box::<Self>::try_new_uninit()?.assume_init();
        Ok(Box::into_raw(boxed_page) as *mut u8)
    }

    /// 释放通过`new_*`分配的物理页
    ///
    /// # 功能说明
    /// 将原始指针重构为`Box`并执行析构，释放物理页内存
    ///
    /// # 参数
    /// - `raw`：由`new_*`函数返回的原始指针
    ///
    /// # 安全性
    /// - `raw`必须是由`new_*`函数分配的有效指针
    /// - 调用后指针立即失效，不得再次使用
    unsafe fn from_raw_and_drop(raw: *mut u8) {
        drop(Box::from_raw(raw as *mut Self));
    }
}

/// 单页大小（4096字节）的内存页结构
///
/// # 内存布局
/// - `#[repr(C, align(4096)]` 确保页对齐
/// - 固定大小：`PGSIZE`（通常4096字节）
#[repr(C, align(4096))]
pub struct RawSinglePage {
    data: [u8; PGSIZE]
}

impl RawPage for RawSinglePage {}

/// 双页大小（8192字节）的内存页结构
///
/// # 内存布局
/// - 固定大小：`PGSIZE * 2`
/// - 其他特性同[`RawSinglePage`]
#[repr(C, align(4096))]
pub struct RawDoublePage {
    data: [u8; PGSIZE*2]
}

impl RawPage for RawDoublePage {}

/// 四页大小（16384字节）的内存页结构
///
/// # 内存布局
/// - 固定大小：`PGSIZE * 4`
/// - 其他特性同[`RawSinglePage`]
#[repr(C, align(4096))]
pub struct RawQuadPage {
    data: [u8; PGSIZE*4]
}

impl RawPage for RawQuadPage {}

/// 表示不同来源的地址，支持用户空间虚拟地址和内核空间指针
///
/// # 变体说明
/// - `Virtual(usize)`：用户空间虚拟地址
/// - `Kernel(*const u8)`：内核空间不可变指针
/// - `KernelMut(*mut u8)`：内核空间可变指针
#[derive(Clone, Copy, Debug)]
pub enum Address {
    Virtual(usize),
    Kernel(*const u8),
    KernelMut(*mut u8),
}

impl Address {
    /// 计算当前地址的偏移量
    ///
    /// # 功能说明
    /// 根据地址类型生成偏移后的新地址，偏移量必须小于`isize::MAX`
    ///
    /// # 参数
    /// - `count`：要偏移的字节数
    ///
    /// # 返回值
    /// 偏移后的新`Address`实例
    ///
    /// # 断言
    /// - 调试模式下验证`count < isize::MAX`
    pub fn offset(self, count: usize) -> Self {
        debug_assert!(count < (isize::MAX) as usize);
        match self {
            Self::Virtual(p) => Self::Virtual(p + count),
            Self::Kernel(p) => Self::Kernel(unsafe { p.offset(count as isize) }),
            Self::KernelMut(p) => Self::KernelMut(unsafe { p.offset(count as isize) }),
        }
    }

    /// 从源地址复制数据到当前地址
    ///
    /// # 功能说明
    /// 根据地址类型执行不同的复制操作：
    /// - 用户空间地址：通过进程的地址空间管理器复制
    /// - 内核不可变指针：禁止写入（触发panic）
    /// - 内核可变指针：直接内存复制
    ///
    /// # 参数
    /// - `src`：源数据起始地址
    /// - `count`：要复制的字节数
    ///
    /// # 返回值
    /// - `Ok(())`：复制成功
    /// - `Err(())`：用户空间复制失败
    ///
    /// # 安全性
    /// - 内核指针操作使用`ptr::copy`，需确保内存区域有效
    /// - 用户空间地址由`copy_out`方法检查有效性
    pub fn copy_out(self, src: *const u8, count: usize) -> Result<(), ()> {
        match self {
            Self::Virtual(dst) => {
                let p = unsafe { CPU_MANAGER.my_proc() };
                p.data.get_mut().copy_out(src, dst, count)
            },
            Self::Kernel(dst) => {
                panic!("cannot copy to a const pointer {:p}", dst)
            },
            Self::KernelMut(dst) => {
                unsafe { ptr::copy(src, dst, count); }
                Ok(())
            },
        }
    }

    /// 从当前地址复制数据到目标地址
    ///
    /// # 功能说明
    /// 根据地址类型执行不同的复制操作：
    /// - 用户空间地址：通过进程的地址空间管理器复制
    /// - 内核指针：直接内存复制
    ///
    /// # 参数
    /// - `dst`：目标地址
    /// - `count`：要复制的字节数
    ///
    /// # 返回值
    /// - `Ok(())`：复制成功
    /// - `Err(())`：用户空间复制失败
    ///
    /// # 安全性
    /// 同`copy_out`
    pub fn copy_in(self, dst: *mut u8, count: usize) -> Result<(), ()> {
        match self {
            Self::Virtual(src) => {
                let p = unsafe { CPU_MANAGER.my_proc() };
                p.data.get_mut().copy_in(src, dst, count)
            },
            Self::Kernel(src) => {
                unsafe { ptr::copy(src, dst, count); }
                Ok(())
            },
            Self::KernelMut(src) => {
                debug_assert!(false);
                unsafe { ptr::copy(src, dst, count); }
                Ok(())
            },
        }
    }
}

/// 向上取整到页边界
///
/// # 功能说明
/// 计算大于等于`address`的最小页对齐地址
#[inline]
pub fn pg_round_up(address: usize) -> usize {
    (address + (PGSIZE - 1)) & !(PGSIZE - 1)
}

/// 向下取整到页边界
///
/// # 功能说明
/// 计算小于等于`address`的最大页对齐地址
#[inline]
pub fn pg_round_down(address: usize) -> usize {
    address & !(PGSIZE - 1)
}
