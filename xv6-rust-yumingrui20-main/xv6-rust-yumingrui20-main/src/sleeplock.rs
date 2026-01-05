//! 睡眠锁模块
//! 提供基于进程休眠/唤醒机制的同步原语，适用于可能长时间持有的锁。
//! 
//! 当锁被占用时，尝试获取锁的进程会进入休眠状态，避免忙等待。

use core::ops::{Deref, DerefMut, Drop};
use core::cell::{Cell, UnsafeCell};

use crate::process::{CPU_MANAGER, PROC_MANAGER};
use crate::spinlock::SpinLock;

/// 睡眠锁结构，提供阻塞式同步机制
///
/// 与自旋锁不同，当锁被占用时，尝试获取的进程会进入休眠状态，
/// 直到锁被释放后被唤醒。这避免了忙等待，适用于可能长时间持有的锁。
///
/// # 类型参数
/// - `T`: 被保护的数据类型
///
/// # 字段说明
/// - `lock`: 内部自旋锁，保护`locked`状态的原子访问
/// - `locked`: 表示锁是否已被占用
/// - `name`: 锁的标识名称，用于调试
/// - `data`: 被保护的数据，通过`UnsafeCell`实现内部可变性
pub struct SleepLock<T: ?Sized> {
    lock: SpinLock<()>,
    locked: Cell<bool>,
    name: &'static str,
    data: UnsafeCell<T>,
}

// 为SleepLock实现Sync，允许跨线程共享（要求T是Send）
unsafe impl<T: ?Sized + Send> Sync for SleepLock<T> {}

// 不需要
// unsafe impl<T: ?Sized + Send> Send for SleepLock<T> {}

impl<T> SleepLock<T> {
    /// 创建一个新的睡眠锁实例
    ///
    /// # 参数
    /// - `data`: 需要被保护的数据
    /// - `name`: 锁的标识名称
    ///
    /// # 返回值
    /// 初始化完成的`SleepLock<T>`实例
    pub const fn new(data: T, name: &'static str) -> Self {
        Self {
            lock: SpinLock::new((), "sleeplock"),
            locked: Cell::new(false),
            name,
            data: UnsafeCell::new(data),
        }
    }
}

impl<T: ?Sized> SleepLock<T> {
    /// 获取睡眠锁（可能阻塞进程）
    ///
    /// # 功能说明
    /// 尝试获取睡眠锁。如果锁已被占用，当前进程将进入休眠状态，
    /// 直到锁被释放后被唤醒。返回守卫对象提供对数据的访问。
    ///
    /// # 流程解释
    /// 1. 获取内部自旋锁保护临界区
    /// 2. 检查`locked`状态：
    ///   - 如果已锁定：调用`sleep()`让当前进程休眠
    ///   - 如果未锁定：设置`locked=true`并返回守卫
    /// 3. 释放内部自旋锁（因已设置locked状态）
    ///
    /// # 返回值
    /// `SleepLockGuard<T>`守卫对象，提供对内部数据的访问
    ///
    /// # 安全性
    /// - 使用`UnsafeCell`获取数据指针，但通过守卫模式保证安全访问
    pub fn lock(&self) -> SleepLockGuard<'_, T> {
        // 获取内部自旋锁（保护locked状态）
        let mut guard = self.lock.lock();

        // 当锁已被占用时循环等待
        while self.locked.get() {
            unsafe {
                // 让当前进程休眠，等待锁释放
                CPU_MANAGER.my_proc().sleep(self.locked.as_ptr() as usize, guard);
            }
            // 被唤醒后重新获取内部锁
            guard = self.lock.lock();
        }

        // 成功获取锁，设置状态
        self.locked.set(true);

        // 释放内部自旋锁（已设置locked状态，无需保护）
        drop(guard);

        // 返回守卫对象
        SleepLockGuard {
            lock: &self,
            data: unsafe { &mut *self.data.get() }
        }
    }

    /// 释放锁（内部方法，由守卫的Drop调用）
    ///
    /// # 流程解释
    /// 1. 获取内部自旋锁
    /// 2. 设置`locked=false`表示锁已释放
    /// 3. 唤醒等待该锁的进程
    /// 4. 释放内部自旋锁
    fn unlock(&self) {
        let guard = self.lock.lock();
        self.locked.set(false);
        self.wakeup();
        drop(guard);
    }

    /// 唤醒等待该锁的进程（内部方法）
    ///
    /// 通过进程管理器唤醒所有在`locked`地址上休眠的进程
    fn wakeup(&self) {
        unsafe {
            PROC_MANAGER.wakeup(self.locked.as_ptr() as usize);
        }
    }
}

/// 睡眠锁守卫，提供对受保护数据的访问
///
/// 当守卫存在时，表示锁已被持有。
/// 守卫离开作用域时自动释放锁，确保锁的释放。
///
/// # 类型参数
/// - `'a`: 守卫的生命周期，绑定到锁的生命周期
/// - `T`: 被保护数据的类型
pub struct SleepLockGuard<'a, T: ?Sized> {
    lock: &'a SleepLock<T>,
    data: &'a mut T,
}

impl<'a, T: ?Sized> Deref for SleepLockGuard<'a, T> {
    type Target = T;
    /// 解引用获取数据的不可变引用
    fn deref(&self) -> &T {
        &*self.data
    }
}

impl<'a, T: ?Sized> DerefMut for SleepLockGuard<'a, T> {
    /// 解引用获取数据的可变引用
    fn deref_mut(&mut self) -> &mut T {
        &mut *self.data
    }
}

impl<'a, T: ?Sized> Drop for SleepLockGuard<'a, T> {
    /// 当守卫离开作用域时自动释放锁
    ///
    /// 通过调用关联睡眠锁的`unlock()`方法实现：
    /// 1. 标记锁为可用状态
    /// 2. 唤醒等待该锁的进程
    fn drop(&mut self) {
        self.lock.unlock();
    }
}
