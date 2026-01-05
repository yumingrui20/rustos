//! 设备驱动模块，包含串口与磁盘的驱动

use core::sync::atomic::AtomicBool;

use crate::{consts::driver::NDEV, mm::Address};

pub mod virtio_disk;
pub mod console;
pub mod uart;

/// 用于表示是否有任何硬件线程触发了 panic。
pub(crate) static PANICKED: AtomicBool = AtomicBool::new(false);

pub static DEVICES: [Option<Device>; NDEV] = [
    /* 0 */   None,
    /* 1 */   Some(Device { read: console::read, write: console::write }),
    /* 2 */   None,
    /* 3 */   None,
    /* 4 */   None,
    /* 5 */   None,
    /* 6 */   None,
    /* 7 */   None,
    /* 8 */   None,
    /* 9 */   None,
];

pub struct Device {
    /// 功能：从 [Address] 读取 count 个字节。
    pub read: fn(Address, u32) -> Result<u32, ()>,
    /// 功能：向 [Address] 写入 count 个字节。
    pub write: fn(Address, u32) -> Result<u32, ()>,
}
