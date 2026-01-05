//! Rust语言双向链表实现

use core::ptr;

/// 双向链表节点结构
///
/// # 内存布局
/// - `#[repr(C)]` 确保C兼容内存布局，避免编译器优化重排
/// - 每个节点包含指向前驱和后继节点的裸指针
///
/// # 安全说明
/// - 所有操作均基于裸指针，调用者需确保指针有效性
/// - 链表操作不管理内存生命周期，需由调用者保证节点有效性
#[repr(C)]
pub struct List {
    prev: *mut List,
    next: *mut List,
}

impl List {
    /// 初始化链表节点
    ///
    /// # 功能说明
    /// 将节点初始化为自环状态，形成空链表
    pub fn init(&mut self) {
        self.prev = self;
        self.next = self;
    }

    /// 将新节点插入链表头部
    ///
    /// # 功能说明
    /// 1. 在指定内存地址创建新节点
    /// 2. 将新节点链接到当前链表头部
    ///
    /// # 参数
    /// - `raw_addr`: 新节点的内存地址（需已分配且对齐）
    ///
    /// # 安全性
    /// - `raw_addr` 必须是有效且对齐的内存地址
    /// - 地址必须满足 `List` 的内存布局要求
    /// - 调用后该地址被接管，需通过链表操作释放
    pub unsafe fn push(&mut self, raw_addr: usize) {
        let raw_list = raw_addr as *mut List;
        ptr::write(raw_list, List {
            prev: self,
            next: self.next,
        });
        self.next.as_mut().unwrap().prev = raw_list;
        self.next = raw_list;
    }

    /// 从链表头部弹出节点
    ///
    /// # 功能说明
    /// 1. 移除链表第一个有效节点
    /// 2. 返回被移除节点的原始地址
    ///
    /// # 返回值
    /// 被移除节点的内存地址(`usize`)
    ///
    /// # 安全性
    /// - 链表不能为空（否则panic）
    /// - 返回的地址对应节点已被移出链表
    /// - 调用者需负责该地址的内存管理
    ///
    /// # Panics
    /// 当链表为空时触发panic
    pub unsafe fn pop(&mut self) -> usize {
        if self.is_empty() {
            panic!("empty pop");
        }
        let raw_addr = self.next as usize;
        self.next.as_mut().unwrap().remove();
        raw_addr
    }

    /// 将当前节点从链表中移除
    ///
    /// # 功能说明
    /// 调整相邻节点的指针，使当前节点脱离链表
    ///
    /// # 安全性
    /// - 调用时当前节点必须已链接在链表中
    /// - 操作后节点仍存在，但不再属于任何链表
    /// - 典型使用场景：
    ///   - 销毁节点前解除链接
    ///   - 将节点移动到其他链表
    pub unsafe fn remove(&mut self) {
        (*self.prev).next = self.next;
        (*self.next).prev = self.prev;
    }

    /// 检查链表是否为空
    ///
    /// # 返回值
    /// - `true`: 链表为空（仅含头节点）
    /// - `false`: 链表至少有一个有效节点
    pub fn is_empty(&self) -> bool {
        ptr::eq(self.next, self)
    }
}
