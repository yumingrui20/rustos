//! 超级块操作

use core::ptr;
use core::mem::{self, MaybeUninit};
use core::sync::atomic::{AtomicBool, Ordering};

use crate::consts::fs::{BPB, FSMAGIC};
use super::{BCACHE, BufData, inode::IPB};

/// 全局超级块实例
///
/// # 安全性
/// - 静态可变变量，需在单线程环境下初始化
/// - 通过`AtomicBool`保证初始化状态同步
pub static mut SUPER_BLOCK: SuperBlock = SuperBlock::uninit();

/// 内存中的超级块副本
///
/// # 设计说明
/// - 封装磁盘上的`RawSuperBlock`结构
/// - 使用`MaybeUninit`延迟初始化
/// - 通过原子标志`initialized`确保线程安全访问
///
/// # 同步保证
/// - 实现`Sync`允许跨线程共享引用
/// - 写操作仅在初始化时发生，之后只读
#[derive(Debug)]
pub struct SuperBlock {
    data: MaybeUninit<RawSuperBlock>,
    initialized: AtomicBool,
}

unsafe impl Sync for SuperBlock {}

impl SuperBlock {
    const fn uninit() -> Self {
        Self {
            data: MaybeUninit::uninit(),
            initialized: AtomicBool::new(false),
        }
    }

    /// 从磁盘设备读取并初始化超级块
    ///
    /// # 功能说明
    /// 1. 从指定设备的第一个块（块号1）读取超级块
    /// 2. 验证文件系统魔数（FSMAGIC）
    /// 3. 将数据复制到内存中的全局超级块
    ///
    /// # 参数
    /// - `dev`: 文件系统所在设备号
    ///
    /// # 安全性
    /// - 必须由第一个常规进程单独调用
    /// - 设备号`dev`必须对应有效的文件系统设备
    ///
    /// # Panics
    /// - 文件系统魔数不匹配时触发panic
    ///
    /// # 初始化流程
    /// 1. 检查对齐要求（调试模式）
    /// 2. 通过缓冲缓存读取块1
    /// 3. 复制数据到内存超级块
    /// 4. 验证魔数
    /// 5. 设置初始化标志
    pub unsafe fn init(&mut self, dev: u32) {
        debug_assert_eq!(mem::align_of::<BufData>() % mem::align_of::<RawSuperBlock>(), 0);
        if self.initialized.load(Ordering::Relaxed) {
            return
        }

        let buf = BCACHE.bread(dev, 1);
        ptr::copy_nonoverlapping(
            buf.raw_data() as *const RawSuperBlock,
            self.data.as_mut_ptr(),
            1,
        );
        if self.data.as_ptr().as_ref().unwrap().magic != FSMAGIC {
            panic!("invalid file system magic num");
        }
        self.initialized.store(true, Ordering::SeqCst);
        drop(buf);

        #[cfg(feature = "verbose_init_info")]
        println!("super block data: {:?}", self.data.as_ptr().as_ref().unwrap());
    }

    /// 获取已初始化的超级块只读引用
    ///
    /// # 前置条件
    /// - 超级块必须已完成初始化
    fn read(&self) -> &RawSuperBlock {
        debug_assert!(self.initialized.load(Ordering::Relaxed));
        unsafe {
            self.data.as_ptr().as_ref().unwrap()
        }
    }

    /// 读取日志区域信息
    ///
    /// # 返回值
    /// 元组`(起始块号, 日志块数量)`
    pub fn read_log(&self) -> (u32, u32) {
        let sb = self.read();
        (sb.logstart, sb.nlog)
    }

    /// 定位索引节点所在的磁盘块
    ///
    /// # 参数
    /// - `inum`: 要查询的索引节点号
    ///
    /// # 返回值
    /// 包含该索引节点的磁盘块号
    ///
    /// # Panics
    /// 当`inum`超出索引节点总数时触发panic
    pub fn locate_inode(&self, inum: u32) -> u32 {
        let sb = self.read();
        if inum >= sb.ninodes {
            panic!("query inum {} larger than maximum inode nums {}", inum, sb.ninodes);
        }
        let blockno = (inum / (IPB as u32)) + sb.inodestart;
        blockno
    }

    /// 获取文件系统索引节点总数
    pub fn inode_size(&self) -> u32 {
        let sb = self.read();
        sb.ninodes
    }

    /// 定位块对应的位图块
    ///
    /// # 功能说明
    /// 给定数据块号，返回管理该块的位图块号
    ///
    /// # 参数
    /// - `blockno`: 要查询的数据块号
    ///
    /// # 返回值
    /// 管理该块的位图块号
    ///
    /// # 计算原理
    /// 位图块号 = 位图起始块 + (块号 / 每块管理的位数)
    pub fn bitmap_blockno(&self, blockno: u32) -> u32 {
        let sb = self.read();
        (blockno / BPB) + sb.bmapstart
    }

    /// 获取文件系统总块数
    pub fn size(&self) -> u32 {
        let sb = self.read();
        sb.size
    }
}

/// 磁盘上的原始超级块结构
///
/// # 内存布局
/// - `#[repr(C)]` 确保C兼容布局
/// - 字段排列与磁盘完全一致
///
/// # 字段说明
/// 所有字段均为大端序存储，需转换成本机字节序使用
#[repr(C)]
#[derive(Debug)]
struct RawSuperBlock {
    magic: u32,      // 文件系统魔数，必须为`FSMAGIC`
    size: u32,       // 文件系统映像总块数
    nblocks: u32,    // 数据块数量（不含元数据）
    ninodes: u32,    // 索引节点总数
    nlog: u32,       // 日志块数量
    logstart: u32,   // 第一个日志块的块号
    inodestart: u32, // 第一个索引节点块的块号
    bmapstart: u32,  // 第一个位图块的块号
}
