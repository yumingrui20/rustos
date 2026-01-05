//! driver for virtio device, only used for disk now
//!
//! from sec 2.6 in https://docs.oasis-open.org/virtio/virtio/v1.1/virtio-v1.1.pdf:
//!     * Descriptor Table - occupies the Descriptor Area
//!     * Available Ring - occupies the Driver Area
//!     * Used Ring - occupies the Device Area
//!
//! NOTE: 4096 in #[repr(C, align(4096))] is PGSIZE

use array_macro::array;

use core::convert::TryFrom;
use core::option::Option;
use core::sync::atomic::{fence, Ordering};
use core::ptr;
use core::convert::TryInto;

use crate::consts::{PGSHIFT, PGSIZE, VIRTIO0, fs::BSIZE};
use crate::fs::Buf;
use crate::spinlock::SpinLock;
use crate::process::{PROC_MANAGER, CPU_MANAGER};

pub static DISK: SpinLock<Disk> = SpinLock::new(Disk::new(), "virtio_disk");

/// VirtIO 磁盘设备内存布局
///
/// # 内存布局
/// - 按页对齐要求设计
/// - 描述符表、可用环、已用环分别位于不同页
/// - 使用填充(pad)确保正确对齐
#[repr(C, align(4096))]
pub struct Disk {
    // 第一页
    pad1: Pad,
    desc: [VQDesc; NUM],
    avail: VQAvail,
    // 另一页
    pad2: Pad,
    used: VQUsed,
    // 结尾
    pad3: Pad,
    free: [bool; NUM],
    used_idx: u16,
    info: [Info; NUM],
    ops: [VirtIOBlkReq; NUM],
}

impl Disk {
    /// 创建新的未初始化磁盘实例
    const fn new() -> Self {
        Self {
            pad1: Pad::new(),
            desc: array![_ => VQDesc::new(); NUM],
            avail: VQAvail::new(),
            pad2: Pad::new(),
            pad3: Pad::new(),
            used: VQUsed::new(),
            free: [false; NUM],
            used_idx: 0,
            info: array![_ => Info::new(); NUM],
            ops: array![_ => VirtIOBlkReq::new(); NUM],
        }
    }

    /// 初始化磁盘设备
    ///
    /// # 功能说明
    /// 执行 VirtIO 设备初始化流程：
    /// 1. 验证设备标识
    /// 2. 设备状态协商
    /// 3. 功能位协商
    /// 4. 配置队列
    ///
    /// # 安全性
    /// - 仅在系统启动时调用一次
    /// - 需要独占访问磁盘结构
    ///
    /// # 初始化步骤
    /// 1. 设备识别与验证
    /// 2. 设置 ACKNOWLEDGE 和 DRIVER 状态位
    /// 3. 功能位协商
    /// 4. 设置 FEATURES_OK 状态
    /// 5. 设置 DRIVER_OK 状态
    /// 6. 配置队列0
    pub unsafe fn init(&mut self) {
        debug_assert_eq!((&self.desc as *const _ as usize) % PGSIZE, 0);
        debug_assert_eq!((&self.used as *const _ as usize) % PGSIZE, 0);
        debug_assert_eq!((&self.free as *const _ as usize) % PGSIZE, 0);
    
        if read(VIRTIO_MMIO_MAGIC_VALUE) != 0x74726976
            || read(VIRTIO_MMIO_VERSION) != 1
            || read(VIRTIO_MMIO_DEVICE_ID) != 2
            || read(VIRTIO_MMIO_VENDOR_ID) != 0x554d4551
        {
            panic!("could not find virtio disk");
        }
    
        // 步骤 1、2、3 - 复位并设置这两个状态位
        let mut status: u32 = 0;
        status |= VIRTIO_CONFIG_S_ACKNOWLEDGE;
        write(VIRTIO_MMIO_STATUS, status);
        status |= VIRTIO_CONFIG_S_DRIVER;
        write(VIRTIO_MMIO_STATUS, status);
    
        // 步骤 4 - 读取特征位并进行协商
        let mut features: u32 = read(VIRTIO_MMIO_DEVICE_FEATURES);
        features &= !(1u32 << VIRTIO_BLK_F_RO);
        features &= !(1u32 << VIRTIO_BLK_F_SCSI);
        features &= !(1u32 << VIRTIO_BLK_F_CONFIG_WCE);
        features &= !(1u32 << VIRTIO_BLK_F_MQ);
        features &= !(1u32 << VIRTIO_F_ANY_LAYOUT);
        features &= !(1u32 << VIRTIO_RING_F_EVENT_IDX);
        features &= !(1u32 << VIRTIO_RING_F_INDIRECT_DESC);
        write(VIRTIO_MMIO_DRIVER_FEATURES, features);
    
        // 步骤 5
        // 设置 FEATURES_OK 位以告知设备特征协商已完成
        status |= VIRTIO_CONFIG_S_FEATURES_OK;
        write(VIRTIO_MMIO_STATUS, status);
    
        // 步骤 6
        // 设置 DRIVER_OK 位以告知设备驱动程序已准备就绪
        // 此时设备处于 “活动” 状态
        status |= VIRTIO_CONFIG_S_DRIVER_OK;
        write(VIRTIO_MMIO_STATUS, status);
    
        write(VIRTIO_MMIO_GUEST_PAGE_SIZE, PGSIZE as u32);
    
        // 初始化队列 0
        write(VIRTIO_MMIO_QUEUE_SEL, 0);
        let max = read(VIRTIO_MMIO_QUEUE_NUM_MAX);
        if max == 0 {
            panic!("virtio disk has no queue 0");
        }
        if max < NUM as u32 {
            panic!("virtio disk max queue short than NUM={}", NUM);
        }
        write(VIRTIO_MMIO_QUEUE_NUM, NUM as u32);
        let pfn: usize = (self as *const Disk as usize) >> PGSHIFT;
        write(VIRTIO_MMIO_QUEUE_PFN, u32::try_from(pfn).unwrap());

        // 释放描述符
        self.free.iter_mut().for_each(|f| *f = true);
    }

    /// 分配三个连续描述符
    ///
    /// # 参数
    /// - `idx`: 输出参数，存储分配的描述符索引
    ///
    /// # 返回值
    /// - `true`: 分配成功
    /// - `false`: 分配失败（资源不足）
    ///
    /// # 注意
    /// 失败时会自动释放已分配的描述符
    fn alloc3_desc(&mut self, idx: &mut [usize; 3]) -> bool {
        for i in 0..idx.len() {
            match self.alloc_desc() {
                Some(ix) => idx[i] = ix,
                None => {
                    for j in 0..i {
                        self.free_desc(j);
                    }
                    return false;
                }
            }
        }
        true
    }

    /// 分配单个描述符
    ///
    /// # 返回值
    /// - `Some(usize)`: 分配的描述符索引
    /// - `None`: 无可用描述符
    fn alloc_desc(&mut self) -> Option<usize> {
        debug_assert_eq!(self.free.len(), NUM);
        for i in 0..NUM {
            if self.free[i] {
                self.free[i] = false;
                return Some(i)
            }
        }
        None
    }

    /// 释放单个描述符
    ///
    /// # 参数
    /// - `i`: 要释放的描述符索引
    ///
    /// # Panics
    /// - 索引超出范围
    /// - 描述符已处于空闲状态
    fn free_desc(&mut self, i: usize) {
        if i >= NUM || self.free[i] {
            panic!("desc index not correct");
        }
        self.desc[i].addr = 0;
        self.desc[i].len = 0;
        self.desc[i].flags = 0;
        self.desc[i].next = 0;
        self.free[i] = true;
        unsafe {
            PROC_MANAGER.wakeup(&self.free[0] as *const bool as usize);
        }
    }

    /// 释放描述符链
    ///
    /// # 功能说明
    /// 遍历并释放通过`VRING_DESC_F_NEXT`链接的描述符链
    ///
    /// # 参数
    /// - `i`: 链的起始描述符索引
    fn free_chain(&mut self, mut i: usize) {
        loop {
            let flag = self.desc[i].flags;
            let next = self.desc[i].next;
            self.free_desc(i);
            if (flag & VRING_DESC_F_NEXT) != 0 {
                i = next as usize;
            } else {
                break;
            }
        }
    }

    /// 磁盘中断处理函数
    ///
    /// # 功能说明
    /// 1. 确认并清除中断状态
    /// 2. 处理已用环中的完成项
    /// 3. 唤醒等待缓冲区操作的进程
    ///
    /// # 调用时机
    /// 由内核陷阱/中断处理器在磁盘发出中断时调用
    pub fn intr(&mut self) {
        unsafe {
            let intr_stat = read(VIRTIO_MMIO_INTERRUPT_STATUS);
            write(VIRTIO_MMIO_INTERRUPT_ACK, intr_stat & 0x3);
        }

        fence(Ordering::SeqCst);

        // 当设备向 used ring 添加一个条目时，它会增加 disk.used->idx 的值
        while self.used_idx != self.used.idx {
            fence(Ordering::SeqCst);
            let id = self.used.ring[self.used_idx as usize % NUM].id as usize;

            if self.info[id].status != 0 {
                panic!("interrupt status");
            }

            let buf_raw_data = self.info[id].buf_channel.clone()
                .expect("virtio disk intr handler not found pre-stored buf channel to wakeup");
            self.info[id].disk = false;
            unsafe { PROC_MANAGER.wakeup(buf_raw_data); }

            self.used_idx += 1;
        }
    }
}

/// 为自旋锁保护的磁盘实例添加扩展方法
impl SpinLock<Disk> {
    /// 执行磁盘读写操作
    ///
    /// # 功能说明
    /// 1. 分配描述符链
    /// 2. 设置请求描述符
    /// 3. 提交到可用环
    /// 4. 通知设备
    /// 5. 等待操作完成
    ///
    /// # 参数
    /// - `buf`: 要读写的缓冲区（可变引用）
    /// - `writing`: 操作类型（true=写，false=读）
    ///
    /// # 处理流程
    /// - 可能阻塞当前进程直到操作完成
    pub fn rw(&self, buf: &mut Buf<'_>, writing: bool) {
        let mut guard = self.lock();
        let buf_raw_data = buf.raw_data_mut();

        let mut idx: [usize; 3] = [0; 3];
        loop {
            if guard.alloc3_desc(&mut idx) {
                break;
            } else {
                unsafe {
                    CPU_MANAGER.my_proc().sleep(&guard.free[0] as *const bool as usize, guard);
                }
                guard = self.lock();
            }
        }

        // 格式化描述符
        // QEMU 的 virtio 块设备会读取它们
        let buf0 = &mut guard.ops[idx[0]];
        buf0.type_ = if writing { VIRTIO_BLK_T_OUT } else { VIRTIO_BLK_T_IN };
        buf0.reserved = 0;
        buf0.sector = (buf.read_blockno() as usize * (BSIZE / 512)) as u64;

        guard.desc[idx[0]].addr = buf0 as *mut _ as u64;
        guard.desc[idx[0]].len = core::mem::size_of::<VirtIOBlkReq>().try_into().unwrap();
        guard.desc[idx[0]].flags = VRING_DESC_F_NEXT;
        guard.desc[idx[0]].next = idx[1].try_into().unwrap();

        guard.desc[idx[1]].addr = buf_raw_data as u64;
        guard.desc[idx[1]].len = BSIZE.try_into().unwrap();
        guard.desc[idx[1]].flags = if writing { 0 } else { VRING_DESC_F_WRITE };
        guard.desc[idx[1]].flags |= VRING_DESC_F_NEXT;
        guard.desc[idx[1]].next = idx[2].try_into().unwrap();

        guard.info[idx[0]].status = 0xff;
        guard.desc[idx[2]].addr = &mut guard.info[idx[0]].status as *mut _ as u64;
        guard.desc[idx[2]].len = 1;
        guard.desc[idx[2]].flags = VRING_DESC_F_WRITE;
        guard.desc[idx[2]].next = 0;

        // 记录缓冲区
        // 当磁盘处理完原始缓冲区数据后，将其取回
        guard.info[idx[0]].disk = true;
        guard.info[idx[0]].buf_channel = Some(buf_raw_data as usize);

        {
            let i = guard.avail.idx as usize % NUM;
            guard.avail.ring[i] = idx[0].try_into().unwrap();
        }

        fence(Ordering::SeqCst);

        guard.avail.idx += 1;

        fence(Ordering::SeqCst);

        unsafe { write(VIRTIO_MMIO_QUEUE_NOTIFY, 0); }

        // 等待磁盘处理缓冲区数据
        while guard.info[idx[0]].disk {
            // 选择原始缓冲区数据作为通道
            unsafe { CPU_MANAGER.my_proc().sleep(buf_raw_data as usize, guard); }
            guard = self.lock();
        }

        let buf_channel = guard.info[idx[0]].buf_channel.take();
        debug_assert_eq!(buf_channel.unwrap(), buf_raw_data as usize);
        guard.free_chain(idx[0]);

        drop(guard);
    }
}

/// 内存填充结构（用于对齐）
#[repr(C, align(4096))]
struct Pad();

impl Pad {
    /// 创建新的填充实例
    const fn new() -> Self {
        Self()
    }
}

#[repr(C, align(16))]
struct VQDesc {
    addr: u64,
    len: u32,
    flags: u16,
    next: u16,
}

impl VQDesc {
    /// 创建新的未初始化描述符
    const fn new() -> Self {
        Self {
            addr: 0,
            len: 0,
            flags: 0,
            next: 0,
        }
    }
}

#[repr(C, align(2))]
struct VQAvail {
    flags: u16,
    idx: u16,
    ring: [u16; NUM],
    unused: u16,
}

impl VQAvail {
    /// 创建新的可用环
    const fn new() -> Self {
        Self {
            flags: 0,
            idx: 0,
            ring: [0; NUM],
            unused: 0,
        }
    }
}

#[repr(C, align(4))]
struct VQUsed {
    flags: u16,
    idx: u16,
    ring: [VQUsedElem; NUM],
}

impl VQUsed {
    /// 创建新的已用环
    const fn new() -> Self {
        Self {
            flags: 0,
            idx: 0,
            ring: array![_ => VQUsedElem::new(); NUM],
        }
    }
}

#[repr(C)]
struct VQUsedElem {
    id: u32,
    len: u32,
}

impl VQUsedElem {
    /// 创建新的已用元素
    const fn new() -> Self {
        Self {
            id: 0,
            len: 0,
        }
    }
}

#[repr(C)]
struct Info {
    /// 磁盘读写操作会将睡眠通道存储在其中。
    /// 磁盘中断操作会检索该通道以唤醒进程。
    buf_channel: Option<usize>,
    status: u8,
    /// 相关的缓冲区是否由磁盘拥有?
    disk: bool,
}

impl Info {
    /// 创建新的元信息
    const fn new() -> Self {
        Self {
            buf_channel: None,
            status: 0,
            disk: false,
        }
    }
}

#[repr(C)]
struct VirtIOBlkReq {
    type_: u32,
    reserved: u32,
    sector: u64,
}

impl VirtIOBlkReq {
    /// 创建新的请求
    const fn new() -> Self {
        Self {
            type_: 0,
            reserved: 0,
            sector: 0,
        }
    }
}


//virtio mmio 控制寄存器的偏移量，来自 qemu 的 virtio_mmio.h
const VIRTIO_MMIO_MAGIC_VALUE: usize = 0x000;
const VIRTIO_MMIO_VERSION: usize = 0x004;
const VIRTIO_MMIO_DEVICE_ID: usize = 0x008;
const VIRTIO_MMIO_VENDOR_ID: usize = 0x00c;
const VIRTIO_MMIO_DEVICE_FEATURES: usize = 0x010;
const VIRTIO_MMIO_DRIVER_FEATURES: usize = 0x020;
const VIRTIO_MMIO_GUEST_PAGE_SIZE: usize = 0x028;
const VIRTIO_MMIO_QUEUE_SEL: usize = 0x030;
const VIRTIO_MMIO_QUEUE_NUM_MAX: usize = 0x034;
const VIRTIO_MMIO_QUEUE_NUM: usize = 0x038;
const VIRTIO_MMIO_QUEUE_ALIGN: usize = 0x03c;
const VIRTIO_MMIO_QUEUE_PFN: usize = 0x040;
const VIRTIO_MMIO_QUEUE_READY: usize = 0x044; 
const VIRTIO_MMIO_QUEUE_NOTIFY: usize = 0x050;
const VIRTIO_MMIO_INTERRUPT_STATUS: usize = 0x060;
const VIRTIO_MMIO_INTERRUPT_ACK: usize = 0x064;
const VIRTIO_MMIO_STATUS: usize = 0x070;

////virtio 状态寄存器位，来自 qemu 的 virtio_config.h
const VIRTIO_CONFIG_S_ACKNOWLEDGE: u32 = 1;
const VIRTIO_CONFIG_S_DRIVER: u32 = 2;
const VIRTIO_CONFIG_S_DRIVER_OK: u32 = 4;
const VIRTIO_CONFIG_S_FEATURES_OK: u32 = 8;

// 设备特征位
const VIRTIO_BLK_F_RO: u8 = 5;
const VIRTIO_BLK_F_SCSI: u8 = 7;
const VIRTIO_BLK_F_CONFIG_WCE: u8 = 11;
const VIRTIO_BLK_F_MQ: u8 = 12;
const VIRTIO_F_ANY_LAYOUT: u8 = 27;
const VIRTIO_RING_F_INDIRECT_DESC: u8 = 28;
const VIRTIO_RING_F_EVENT_IDX: u8 = 29;

// 虚拟环描述符标志位
const VRING_DESC_F_NEXT: u16 = 1; // 与另一个描述符链接
const VRING_DESC_F_WRITE: u16 = 2; // 设备写入（相对于读取）

// 用于磁盘操作
const VIRTIO_BLK_T_IN: u32 = 0; // 读磁盘
const VIRTIO_BLK_T_OUT: u32 = 1; // 写磁盘

//这么多 virtio 描述符必须是 2 的幂
const NUM: usize = 8;

#[inline]
unsafe fn read(offset: usize) -> u32 {
    let src = (Into::<usize>::into(VIRTIO0) + offset) as *const u32;
    ptr::read_volatile(src)
}

#[inline]
unsafe fn write(offset: usize, data: u32) {
    let dst = (Into::<usize>::into(VIRTIO0) + offset) as *mut u32;
    ptr::write_volatile(dst, data);
}
