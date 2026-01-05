//! 自旋锁模块
//! 自旋锁将数据包裹在自身内部以保护这些数据。

use core::cell::{Cell, UnsafeCell};
use core::ops::{Deref, DerefMut, Drop};
use core::sync::atomic::{fence, AtomicBool, Ordering};
use core::ptr::addr_of_mut;

use crate::process::{CpuManager, pop_off, push_off};

/// 表示一个自旋锁结构，用于在多核环境下保护共享数据。
///
/// `SpinLock` 提供了互斥访问内部数据的能力，通过忙等待（busy-waiting）实现锁机制。
/// 当锁被占用时，尝试获取锁的CPU将在循环中等待，直到锁被释放。
/// 该锁还跟踪持有锁的CPU ID，用于调试和死锁检测。
///
/// # 类型参数
/// - `T`: 被保护的数据类型，可以是任意大小（`?Sized`）。
///
/// # 字段说明
/// - `lock`: 原子布尔值，表示锁的状态（`false`=未锁定，`true`=已锁定）；
/// - `name`: 锁的名称，用于调试和标识；
/// - `cpuid`: 当前持有锁的CPU ID（-1表示无CPU持有）；
/// - `data`: 被保护的数据，通过`UnsafeCell`实现内部可变性。
#[derive(Debug)]
pub struct SpinLock<T: ?Sized> {
    lock: AtomicBool,
    name: &'static str,
    cpuid: Cell<isize>,
    data: UnsafeCell<T>,
}

// 为SpinLock实现Sync trait，允许跨线程共享（要求T是Send）
unsafe impl<T: ?Sized + Send> Sync for SpinLock<T> {}
// 这对于 xv6-rust 的自旋锁来说可能不需要？尽管这在 std 库和 spin 库中都有实现。
// unsafe impl<T: ?Sized + Send> Send for SpinLock<T> {}

impl<T> SpinLock<T> {
    /// 创建一个新的自旋锁实例。
    ///
    /// # 功能说明
    /// 初始化自旋锁的所有字段，包括原子锁状态、名称、CPU ID和被保护数据。
    ///
    /// # 参数
    /// - `data`: 需要被保护的数据；
    /// - `name`: 锁的标识名称，用于调试。
    ///
    /// # 返回值
    /// 返回初始化完成的 `SpinLock<T>` 实例。
    pub const fn new(data: T, name: &'static str) -> Self {
        Self {
            lock: AtomicBool::new(false),
            name,
            cpuid: Cell::new(-1),
            data: UnsafeCell::new(data),
        }
    }

    /// 初始化自旋锁的名称字段。
    ///
    /// # 功能说明
    /// 用于在内存已分配但未完全初始化的场景下设置锁名称。
    /// 通常在`create`函数中配合`Arc::try_new_zeroed`使用。
    ///
    /// # 参数
    /// - `lock`: 指向未初始化锁的原始指针；
    /// - `name`: 要设置的锁名称。
    ///
    /// # 安全性
    /// - 调用者必须确保此时只有一个线程可以访问该锁；
    /// - 必须确保`lock`指向有效的内存地址；
    /// - 此函数仅用于初始化阶段，正常创建锁应使用`new()`。
    #[inline(always)]
    pub unsafe fn init_name(lock: *mut Self, name: &'static str) {
        addr_of_mut!((*lock).name).write(name);
    }
}

impl<T: ?Sized> SpinLock<T> {
    /// 获取自旋锁并返回一个守卫对象。
    ///
    /// # 功能说明
    /// 通过忙等待获取锁的所有权，返回一个守卫对象。
    /// 守卫对象实现了`Deref`和`DerefMut`，允许直接访问被保护数据。
    /// 当守卫对象离开作用域时，自动释放锁。
    ///
    /// # 流程解释
    /// 1. 调用`push_off()`禁用中断（防止死锁）；
    /// 2. 检查是否已持有锁（防止重入）；
    /// 3. 通过原子操作忙等待直到获取锁；
    /// 4. 设置内存屏障确保操作顺序；
    /// 5. 记录当前CPU ID；
    /// 6. 返回守卫对象。
    ///
    /// # 示例
    /// ```ignore
    /// let lock = SpinLock::new(0, "test");
    /// {
    ///     let mut guard = lock.lock(); // 获取锁
    ///     *guard = 42; // 修改受保护数据
    /// } // 守卫离开作用域，自动释放锁
    /// ```
    ///
    /// # 返回值
    /// 返回`SpinLockGuard<T>`守卫对象，提供对内部数据的访问。
    pub fn lock(&self) -> SpinLockGuard<'_, T> {
        self.acquire();
        SpinLockGuard {
            lock: &self,
            data: unsafe { &mut *self.data.get() },
        }
    }

    /// 检查当前CPU是否持有此锁（内部方法）。
    ///
    /// # 功能说明
    /// 验证当前CPU是否持有该锁，用于调试和防止重入。
    ///
    /// # 前提条件
    /// - 中断必须已禁用（由`push_off`保证）；
    /// 
    /// # 参数
    /// 无
    ///
    /// # 返回值
    /// - `true`：当前CPU持有此锁；
    /// - `false`：当前CPU未持有此锁。
    ///
    /// # 安全性
    /// - 必须在禁用中断的上下文中调用；
    /// - 访问`cpuid`字段是安全的，因为中断已禁用。
    unsafe fn holding(&self) -> bool {
        self.lock.load(Ordering::Relaxed) && (self.cpuid.get() == CpuManager::cpu_id() as isize)
    }

    /// 获取锁的核心实现（内部方法）。
    ///
    /// # 流程解释
    /// 1. 调用`push_off()`禁用中断；
    /// 2. 检查是否已持有锁（防止死锁）；
    /// 3. 使用原子比较交换（CAS）忙等待获取锁；
    /// 4. 获取成功后设置内存屏障；
    /// 5. 记录当前CPU ID。
    ///
    /// # 注意
    /// 此方法不返回守卫对象，仅供内部使用。
    fn acquire(&self) {
        push_off();
        if unsafe { self.holding() } {
            panic!("spinlock {} acquire", self.name);
        }
        while self.lock.compare_exchange(false, true,
            Ordering::Acquire, Ordering::Acquire).is_err() {}
        fence(Ordering::SeqCst);
        unsafe { self.cpuid.set(CpuManager::cpu_id() as isize) };
    }

    /// 释放锁的核心实现（内部方法）。
    ///
    /// # 流程解释
    /// 1. 验证当前CPU确实持有锁；
    /// 2. 清除CPU ID记录；
    /// 3. 设置内存屏障确保操作顺序；
    /// 4. 原子存储`false`释放锁；
    /// 5. 调用`pop_off()`恢复中断状态。
    ///
    /// # 注意
    /// 此方法不直接对外暴露，通过守卫的`Drop`实现自动调用。
    fn release(&self) {
        if unsafe { !self.holding() } {
            panic!("spinlock {} release", self.name);
        }
        self.cpuid.set(-1);
        fence(Ordering::SeqCst);
        self.lock.store(false, Ordering::Release);
        pop_off();
    }
    
    /// 手动释放锁的特殊接口。
    ///
    /// # 功能说明
    /// 提供一种不通过守卫对象释放锁的方式，用于特殊场景如`fork_ret()`。
    /// 正常情况下应使用守卫模式自动管理锁生命周期。
    ///
    /// # 安全性
    /// - 调用者必须确保当前CPU确实持有该锁；
    /// - 释放后不得再访问受保护数据；
    /// - 此方法仅用于特定内核路径（如进程创建）。
    pub unsafe fn unlock(&self) {
        self.release();
    }
}

/// 自旋锁守卫对象，提供对受保护数据的访问。
///
/// 当守卫对象存在时，表示锁已被持有。
/// 守卫离开作用域时自动释放锁，确保锁的释放。
///
/// # 类型参数
/// - `'a`: 守卫的生命周期，绑定到锁的生命周期；
/// - `T`: 被保护数据的类型。
pub struct SpinLockGuard<'a, T: ?Sized> {
    lock: &'a SpinLock<T>,
    data: &'a mut T,
}

impl<'a, T: ?Sized> Deref for SpinLockGuard<'a, T> {
    type Target = T;

    /// 解引用获取数据的不可变引用。
    ///
    /// # 安全性
    /// 守卫存在时锁已被持有，因此访问是安全的。
    fn deref(&self) -> &T {
        &*self.data
    }
}

impl<'a, T: ?Sized> DerefMut for SpinLockGuard<'a, T> {

    /// 解引用获取数据的可变引用。
    ///
    /// # 安全性
    /// 守卫存在时锁已被持有，因此访问是安全的。
    fn deref_mut(&mut self) -> &mut T {
        &mut *self.data
    }
}

impl<'a, T: ?Sized> Drop for SpinLockGuard<'a, T> {
    /// 当守卫离开作用域时自动释放锁。
    ///
    /// # 流程解释
    /// 调用关联自旋锁的`release()`方法释放锁，
    /// 并恢复中断状态（通过`pop_off`）
    fn drop(&mut self) {
        self.lock.release();
    }
}

impl<'a, T> SpinLockGuard<'a, T> {
    /// 检查当前CPU是否持有此锁。
    ///
    /// # 前提条件
    /// - 中断必须已禁用；
    ///
    /// # 返回值
    /// - `true`：当前CPU持有此锁；
    /// - `false`：当前CPU未持有此锁。
    ///
    /// # 安全性
    /// - 必须在禁用中断的上下文中调用；
    /// - 守卫存在时通常应持有锁，此方法用于调试验证。
    pub unsafe fn holding(&self) -> bool {
        self.lock.holding()
    }
}

/// 从spin crate借鉴 (https://crates.io/crates/spin)
#[cfg(feature = "unit_test")]
pub mod tests {
    use super::*;

    /// 基础功能测试：验证锁的获取和释放。
    ///
    /// # 测试点
    /// 1. 创建新锁；
    /// 2. 连续两次获取锁（应成功，因在单线程上下文）；
    /// 3. 自动释放机制验证。
    pub fn smoke() {
        let m = SpinLock::new((), "smoke");
        m.lock();
        m.lock();
    }
}
