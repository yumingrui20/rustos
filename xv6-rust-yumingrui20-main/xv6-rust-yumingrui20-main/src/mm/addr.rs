//! 提供物理地址与虚拟地址包装

use core::convert::TryFrom;
use core::result::Result;
use core::ops::{Add, Sub};

use crate::consts::{PGMASK, PGMASKLEN, PGSHIFT, PGSIZE, PHYSTOP, MAXVA, ConstAddr};

/// 地址类型通用接口
///
/// 定义物理地址和虚拟地址共有的操作方法，
/// 包括页对齐调整、地址转换等。
pub trait Addr {
    /// 获取内部地址值的不可变引用
    fn data_ref(&self) -> &usize;

    /// 获取内部地址值的可变引用
    fn data_mut(&mut self) -> &mut usize;

    /// 向上取整到页边界
    #[inline]
    fn pg_round_up(&mut self) {
        *self.data_mut() = (*self.data_mut() + PGSIZE - 1) & !(PGSIZE - 1)
    }

    /// 向下取整到页边界
    #[inline]
    fn pg_round_down(&mut self) {
        *self.data_mut() = *self.data_mut() & !(PGSIZE - 1)
    }

    /// 增加一页大小（PGSIZE）
    ///
    /// # 注意
    /// 不检查地址是否合法，调用者需确保操作后地址有效
    #[inline]
    fn add_page(&mut self) {
        *self.data_mut() += PGSIZE;
    }

    /// 获取地址的usize表示
    #[inline]
    fn as_usize(&self) -> usize {
        *self.data_ref()
    }

    /// 转换为只读裸指针
    ///
    /// # 安全性
    /// 返回的指针需在有效生命周期内使用
    #[inline]
    fn as_ptr(&self) -> *const u8 {
        *self.data_ref() as *const u8
    }

    /// 转换为可变裸指针
    ///
    /// # 安全性
    /// 调用者需确保指针修改不会破坏内存安全
    #[inline]
    fn as_mut_ptr(&mut self) -> *mut u8 {
        *self.data_mut() as *mut u8
    }
}

/// 物理地址封装类型
///
/// # 内存布局
/// - `#[repr(C)]` 确保C兼容内存布局
/// - 存储`usize`类型的原始物理地址
///
/// # 合法性保证
/// - 地址必须页对齐（通过`TryFrom`实现检查）
/// - 地址值不超过`PHYSTOP`定义的上限
#[repr(C)]
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub struct PhysAddr(usize);

impl Addr for PhysAddr {
    #[inline]
    fn data_ref(&self) -> &usize {
        &self.0
    }

    #[inline]
    fn data_mut(&mut self) -> &mut usize {
        &mut self.0
    }
}

impl PhysAddr {
    /// 从原始usize值构造物理地址
    ///
    /// # 安全性
    /// 调用者必须确保：
    /// 1. `raw`是有效的物理地址
    /// 2. `raw`满足页对齐要求
    /// 3. `raw`不超过`PHYSTOP`限制
    #[inline]
    pub unsafe fn from_raw(raw: usize) -> Self {
        Self(raw)
    }

    /// 解封装获取原始物理地址
    #[inline]
    pub fn into_raw(self) -> usize {
        self.0
    }
}

impl TryFrom<usize> for PhysAddr {
    type Error = &'static str;

    /// 尝试从usize创建物理地址
    ///
    /// # 参数
    /// - `addr`: 原始物理地址值
    ///
    /// # 返回值
    /// - `Ok(PhysAddr)`: 地址合法且满足约束
    /// - `Err(&str)`: 地址非法（未对齐或超出范围）
    ///
    /// # 检查条件
    /// 1. 地址必须页对齐（`addr % PGSIZE == 0`）
    /// 2. 地址不超过`PHYSTOP`定义的上限
    fn try_from(addr: usize) -> Result<Self, Self::Error> {
        if addr % PGSIZE != 0 {
            return Err("PhysAddr addr not aligned");
        }
        if addr > usize::from(PHYSTOP) {
            return Err("PhysAddr addr bigger than PHYSTOP");
        }
        Ok(PhysAddr(addr))
    }
}

impl From<ConstAddr> for PhysAddr {
    /// 从编译时常量地址转换
    fn from(const_addr: ConstAddr) -> Self {
        Self(const_addr.into())
    }
}

/// 虚拟地址封装类型
///
/// # 内存布局
/// - `#[repr(C)]` 确保C兼容内存布局
/// - 存储`usize`类型的原始虚拟地址
///
/// # Sv39规范保证
/// 地址值保证满足RISC-V Sv39虚拟内存规范：
/// - 63-39位必须为0（避免符号扩展问题）
/// - 最大地址不超过`MAXVA`（通常为1<<38）
#[repr(C)]
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub struct VirtAddr(usize);

impl Addr for VirtAddr {
    #[inline]
    fn data_ref(&self) -> &usize {
        &self.0
    }

    #[inline]
    fn data_mut(&mut self) -> &mut usize {
        &mut self.0
    }
}

impl VirtAddr {
    /// 从原始usize值构造虚拟地址
    ///
    /// # 安全性
    /// 调用者必须确保：
    /// 1. `raw`是有效的虚拟地址
    /// 2. `raw`满足Sv39规范（高位为0）
    #[inline]
    pub unsafe fn from_raw(raw: usize) -> Self {
        Self(raw)
    }

    /// 解封装获取原始虚拟地址
    #[inline]
    pub fn into_raw(self) -> usize {
        self.0
    }

    /// 获取指定层级的虚拟页号(VPN)
    ///
    /// # 参数
    /// - `level`: 页表层级（0=4KB页, 1=2MB大页, 2=1GB大页）
    ///
    /// # 返回值
    /// 指定层级的9位VPN值
    ///
    /// # 注意
    /// 仅接受0-2范围内的层级参数
    #[inline]
    pub fn page_num(&self, level: usize) -> usize {
        (self.0 >> (PGSHIFT + level * PGMASKLEN)) & PGMASK
    }
}

impl TryFrom<usize> for VirtAddr {
    type Error = &'static str;

    /// 尝试从usize创建虚拟地址
    ///
    /// # 参数
    /// - `addr`: 原始虚拟地址值
    ///
    /// # 返回值
    /// - `Ok(VirtAddr)`: 地址满足Sv39规范
    /// - `Err(&str)`: 地址超出MAXVA限制
    ///
    /// # 检查条件
    /// 地址值必须小于等于`MAXVA`
    fn try_from(addr: usize) -> Result<Self, Self::Error> {
        if addr > MAXVA.into() {
            Err("value for VirtAddr should be smaller than 1<<38")
        } else {
            Ok(Self(addr))
        }
    }
}

impl From<ConstAddr> for VirtAddr {
    /// 从编译时常量地址转换
    fn from(const_addr: ConstAddr) -> Self {
        Self(const_addr.into())
    }
}

impl Add for VirtAddr {
    type Output = Self;

    /// 虚拟地址加法
    ///
    /// # 注意
    /// 不检查结果地址的有效性
    fn add(self, other: Self) -> Self {
        Self(self.0 + other.0)
    }
}

impl Sub for VirtAddr {
    type Output = Self;

    /// 虚拟地址减法
    ///
    /// # 注意
    /// 不检查结果地址的有效性
    fn sub(self, other: Self) -> Self {
        Self(self.0 - other.0)
    }
}
