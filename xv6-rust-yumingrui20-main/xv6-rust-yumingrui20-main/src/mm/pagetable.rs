//! 页表配置与管理

use array_macro::array;

use alloc::boxed::Box;
use core::{cmp::min, convert::TryFrom};
use core::ptr;

use crate::consts::{PGSHIFT, PGSIZE, SATP_SV39, SV39FLAGLEN, USERTEXT, TRAMPOLINE, TRAPFRAME};
use super::{Addr, PhysAddr, RawPage, RawSinglePage, VirtAddr, pg_round_up};

bitflags! {
    /// 内存页表项权限标志（Page Table Entry Flags）
    ///
    /// 该结构体定义了页表项中的各种权限和状态标志，
    /// 用于控制虚拟内存页的访问权限和管理信息。  
    /// 在xv6教学操作系统内核中，`PteFlag` 用于设置页表项的读/写/执行权限，  
    /// 是否有效，用户态访问权限，以及辅助标志（如访问和修改标志）。
    pub struct PteFlag: usize {
        /// 有效位（Valid）  
        /// 标记该页表项是否有效，若无效则该项不代表有效映射。
        const V = 1 << 0;
        
        /// 读权限（Readable）  
        /// 允许访问该页进行读取操作。
        const R = 1 << 1;
        
        /// 写权限（Writable）  
        /// 允许访问该页进行写入操作。
        const W = 1 << 2;
        
        /// 执行权限（Executable）  
        /// 允许该页内存中的指令被执行。
        const X = 1 << 3;
        
        /// 用户态访问权限（User）  
        /// 允许用户模式访问该页。
        const U = 1 << 4;
        
        /// 全局位（Global）  
        /// 标记该页为全局页，不随地址空间切换而失效。
        const G = 1 << 5;
        
        /// 访问位（Accessed）  
        /// CPU硬件设置，表示该页曾被访问过。
        const A = 1 << 6;
        
        /// 脏位（Dirty）  
        /// CPU硬件设置，表示该页曾被写入。
        const D = 1 << 7;
        
        /// 保留供软件使用位（Reserved for Software）  
        /// 两位宽的软件标志位，供操作系统使用。
        const RSW = 0b11 << 8;
    }
}

/// 页表项结构体（PageTableEntry）
///
/// 该结构体表示单个页表项，封装了页表项的原始数据。  
/// 在内核的虚拟内存管理中，页表项用于描述虚拟页到物理页的映射信息，  
/// 包括物理页地址和权限标志等。该结构体提供对页表项数据的封装和操作接口。
#[repr(C)]
#[derive(Debug)]
pub struct PageTableEntry {
    /// 页表项的原始数据，包含物理页帧号和权限标志等信息，  
    /// 具体位域布局遵循目标架构页表格式规范（如 RISC-V Sv39）。  
    data: usize,
}

impl PageTableEntry {
    #[inline]
    pub fn is_valid(&self) -> bool {
        (self.data & (PteFlag::V.bits())) > 0
    }

    #[inline]
    fn is_leaf(&self) -> bool {
        let flag_bits = self.data & (PteFlag::R|PteFlag::W|PteFlag::X).bits();
        !(flag_bits == 0)
    }

    #[inline]
    fn is_user(&self) -> bool {
        (self.data & (PteFlag::U.bits())) > 0
    }

    #[inline]
    fn clear_user(&mut self) {
        self.data &= !PteFlag::U.bits()
    }

    #[inline]
    fn as_page_table(&self) -> *mut PageTable {
        ((self.data >> SV39FLAGLEN) << PGSHIFT) as *mut PageTable
    }

    #[inline]
    pub fn as_phys_addr(&self) -> PhysAddr {
        unsafe { PhysAddr::from_raw((self.data >> SV39FLAGLEN) << PGSHIFT) }
    }

    #[inline]
    fn write_zero(&mut self) {
        self.data = 0;
    }

    #[inline]
    fn write(&mut self, pa: PhysAddr) {
        self.data = ((pa.as_usize() >> PGSHIFT) << SV39FLAGLEN) | (PteFlag::V.bits());
    }

    #[inline]
    fn write_perm(&mut self, pa: PhysAddr, perm: PteFlag) {
        self.data = ((pa.as_usize() >> PGSHIFT) << SV39FLAGLEN) | (perm | PteFlag::V).bits()
    }

    #[inline]
    fn read_perm(&self) -> PteFlag {
        PteFlag::from_bits_truncate(self.data)
    }

    /// # 功能说明
    /// 尝试克隆当前页表项所映射的物理页内容，返回一块新的内存页指针，  
    /// 该内存页保存原物理页的完整数据的副本。  
    /// 该方法用于复制页表项对应的内存页，常用于页表项复制或写时复制（COW）场景。
    ///
    /// # 参数
    /// - `&self`：当前页表项的引用。
    ///
    /// # 返回值
    /// - `Ok(*mut u8)`：指向新分配且初始化完成的物理页内存起始地址的裸指针。  
    /// - `Err(())`：在新内存页分配失败时返回错误。
    ///
    /// # 可能的错误
    /// - 当页表项无效（`is_valid()` 返回 false）时，函数会 panic。  
    /// - 新物理页内存分配失败时返回错误 `Err(())`。
    ///
    /// # 安全性
    /// - 函数为 `unsafe`，调用者必须确保页表项有效且所指内存页可安全读取。  
    /// - 复制操作使用裸指针直接访问内存，可能导致未定义行为，调用时需保证内存有效且不重叠。  
    /// - 返回的裸指针由调用者管理，需妥善释放以避免内存泄漏或悬挂指针。
    unsafe fn try_clone(&self) -> Result<*mut u8, ()> {
        if !self.is_valid() {
            panic!("cloning not valid pte");
        }
        let pa = self.as_phys_addr().into_raw();
        let mem = RawSinglePage::try_new_uninit().map_err(|_| ())?;
        ptr::copy_nonoverlapping(pa as *const u8, mem, PGSIZE);
        Ok(mem)
    }

    /// # 功能说明
    /// 释放当前页表项指向的非叶子页表结构，
    /// 将该页表项所占用的子页表内存释放，并清除该页表项数据。
    /// 该函数仅适用于非叶子节点的页表项，叶子节点页表项不允许调用该函数。
    ///
    /// # 参数
    /// - `&mut self`：当前页表项的可变引用。
    ///
    /// # 返回值
    /// 无返回值。
    ///
    /// # 可能的错误
    /// - 当尝试释放叶子页表项时，函数会触发 panic，避免非法释放。
    ///
    /// # 安全性
    /// - 函数中调用了 `unsafe` 代码将裸指针转换为 `Box` 进行释放，
    ///   调用者需保证页表项指向的地址为有效且合法的页表内存。  
    /// - 函数修改页表项数据，需保证调用时页表状态一致且无并发冲突。
    fn free(&mut self) {
        if self.is_valid() {
            if !self.is_leaf() {
                drop(unsafe { Box::from_raw(self.as_page_table()) });
                self.data = 0;
            } else {
                panic!("freeing a pte leaf")
            }
        }
    }
}

/// 页表结构体（PageTable）
///
/// 该结构体表示一个内存页大小（通常4KB）的页表页，
/// 包含512个页表项（`PageTableEntry`），用于实现多级页表中的一级。  
/// 在模仿xv6的Rust教学操作系统内核中，`PageTable` 管理虚拟地址空间的映射，  
/// 每个页表页对应虚拟地址的一个连续区间，支持页表项的索引和递归查询。
#[repr(C, align(4096))]
pub struct PageTable {
    /// 512个页表项数组，每个项对应一个虚拟页映射信息，  
    /// 其数量512对应于RISC-V Sv39架构的页表大小，  
    /// 支持对虚拟地址空间的细粒度映射和权限管理。
    data: [PageTableEntry; 512],
}

impl PageTable {
    pub const fn empty() -> Self {
        Self {
            data: array![_ => PageTableEntry { data: 0 }; 512],
        }
    }

    /// Convert the page table to be the usize
    /// that can be written in satp register
    pub fn as_satp(&self) -> usize {
        SATP_SV39 | ((self as *const PageTable as usize) >> PGSHIFT)
    }

    /// # 功能说明
    /// 在当前页表及其多级子页表中建立从虚拟地址 `va` 开始、长度为 `size` 字节的连续映射，  
    /// 将虚拟地址区间映射到物理地址 `pa` 开始的对应区间，权限由 `perm` 指定。  
    /// 该函数会自动对齐虚拟地址区间，并递归分配多级页表项，防止重复映射。
    ///
    /// # 参数
    /// - `&mut self`：页表的可变引用。  
    /// - `va`：起始虚拟地址。  
    /// - `size`：映射大小，单位字节。  
    /// - `pa`：起始物理地址。  
    /// - `perm`：映射权限标志，类型为 `PteFlag`。
    ///
    /// # 返回值
    /// - `Ok(())`：映射成功。  
    /// - `Err(&'static str)`：映射失败，通常因页表内存分配不足。
    ///
    /// # 可能的错误
    /// - 若映射区间内某页已存在有效映射，函数会 panic 并打印调试信息。  
    /// - 页表分配失败导致无法递归创建新页表时返回错误。  
    /// - 输入虚拟地址加大小计算时发生溢出，导致 `VirtAddr::try_from` 失败。
    ///
    /// # 安全性
    /// - 该函数假设调用者提供的地址和大小均有效且正确对齐（函数会自动对齐虚拟地址范围）。  
    /// - 由于涉及对多级页表结构的修改，调用时需保证独占访问，避免并发冲突。  
    /// - panic 会导致内核异常，应在调用前确保地址区间不重复映射。
    pub fn map_pages(
        &mut self,
        mut va: VirtAddr,
        size: usize,
        mut pa: PhysAddr,
        perm: PteFlag,
    ) -> Result<(), &'static str> {
        let mut last = VirtAddr::try_from(va.as_usize() + size)?;
        va.pg_round_down();
        last.pg_round_up();

        while va != last {
            match self.walk_alloc(va) {
                Some(pte) => {
                    if pte.is_valid() {
                        println!(
                            "va: {:#x}, pa: {:#x}, pte: {:#x}",
                            va.as_usize(),
                            pa.as_usize(),
                            pte.data
                        );
                        panic!("remap");
                    }
                    pte.write_perm(pa, perm);
                    va.add_page();
                    pa.add_page();
                }
                None => {
                    return Err("PageTable.map_pages: \
                    not enough memory for new page table")
                }
            }
        }

        Ok(())
    }

    /// # 功能说明
    /// 递归遍历或分配多级页表中的页表项，定位并返回虚拟地址 `va` 对应的最低级（叶子级）页表项的可变引用。  
    /// 如果中间级页表项无效，会动态分配并初始化新的页表页，保证页表路径完整。  
    /// 本函数支持在多级页表中为指定虚拟地址分配页表结构，方便建立页映射。
    ///
    /// # 参数
    /// - `&mut self`：当前页表的可变引用。  
    /// - `va`：欲访问的虚拟地址。
    ///
    /// # 返回值
    /// - `Some(&mut PageTableEntry)`：成功返回指向叶子页表项的可变引用。  
    /// - `None`：动态分配新页表失败时返回 `None`。
    ///
    /// # 可能的错误
    /// - 动态分配零初始化页表页失败导致返回 `None`。  
    /// - 虚拟地址转换页号过程中可能存在边界或非法地址问题（调用者需保证地址合法）。
    /// - 使用裸指针转换存在潜在不安全风险，若页表页无效可能导致未定义行为。
    ///
    /// # 安全性
    /// - 函数内部使用了大量 `unsafe` 代码块，调用裸指针和未初始化内存，调用者需确保调用环境安全。  
    /// - 分配的页表页必须正确释放，否则会导致内存泄漏。  
    /// - 修改页表结构必须保证单线程或同步，避免并发写入冲突。  
    /// - 返回的可变引用指向底层内存，调用者应确保使用时内存有效且无数据竞争。
    fn walk_alloc(&mut self, va: VirtAddr) -> Option<&mut PageTableEntry> {
        let mut pgt = self as *mut PageTable;
        for level in (1..=2).rev() {
            let pte = unsafe { &mut pgt.as_mut().unwrap().data[va.page_num(level)] };

            if pte.is_valid() {
                pgt = pte.as_page_table();
            } else {
                let zerod_pgt = unsafe { Box::<Self>::try_new_zeroed().ok()?.assume_init() };
                pgt = Box::into_raw(zerod_pgt);
                pte.write(PhysAddr::try_from(pgt as usize).unwrap());
            }
        }
        unsafe { Some(&mut pgt.as_mut().unwrap().data[va.page_num(0)]) }
    }

    /// 与 [walk_alloc] 功能相同，
    /// 但如果页表不存在时不会分配新的页表。
    fn walk_mut(&mut self, va: VirtAddr) -> Option<&mut PageTableEntry> {
        let mut pgt = self as *mut PageTable;
        for level in (1..=2).rev() {
            let pte = unsafe { &mut pgt.as_mut().unwrap().data[va.page_num(level)] };

            if pte.is_valid() {
                pgt = pte.as_page_table();
            } else {
                return None
            }
        }
        unsafe { Some(&mut pgt.as_mut().unwrap().data[va.page_num(0)]) }
    }

    /// 与 [walk_mut] 功能相同，
    /// 但返回的是不可变引用（非可变的页表项引用）。
    pub fn walk(&self, va: VirtAddr) -> Option<&PageTableEntry> {
        let mut pgt = self as *const PageTable;
        for level in (1..=2).rev() {
            let pte = unsafe { &pgt.as_ref().unwrap().data[va.page_num(level)] };

            if pte.is_valid() {
                pgt = pte.as_page_table();
            } else {
                return None
            }
        }
        unsafe { Some(&pgt.as_ref().unwrap().data[va.page_num(0)]) }
    }

    /// 与 [walk_addr] 功能相同，
    /// 但返回的物理地址指向的数据可以被修改。
    pub fn walk_addr_mut(&mut self, va: VirtAddr)
        -> Result<PhysAddr, &'static str>
    {
        match self.walk_mut(va) {
            Some(pte) => {
                if !pte.is_valid() {
                    Err("pte not valid")
                } else if !pte.is_user() {
                    Err("pte not mapped for user")
                } else {
                    Ok(pte.as_phys_addr())
                }
            }
            None => {
                Err("va not mapped")
            }
        }
    }

    /// # 功能说明
    /// 根据虚拟地址 `va` 查找对应的物理地址。  
    /// 该函数通过页表递归查找页表项，验证页表项是否有效且允许用户态访问，  
    /// 若满足条件则返回物理地址，否则返回错误信息。
    ///
    /// # 参数
    /// - `&self`：页表的不可变引用。  
    /// - `va`：欲查询的虚拟地址。
    ///
    /// # 返回值
    /// - `Ok(PhysAddr)`：虚拟地址对应的物理地址。  
    /// - `Err(&'static str)`：查找失败的错误信息，可能是页表项无效、非用户映射或虚拟地址未映射。
    ///
    /// # 可能的错误
    /// - 页表项无效时返回 `"pte not valid"`。  
    /// - 页表项不允许用户态访问时返回 `"pte not mapped for user"`。  
    /// - 虚拟地址未映射时返回 `"va not mapped"`。
    ///
    /// # 安全性
    /// - 函数为安全接口，不涉及 `unsafe` 操作。  
    /// - 调用者需保证 `va` 是合法的虚拟地址，避免频繁错误调用。  
    /// - 函数不修改页表，适合查询用途，线程安全。
    pub fn walk_addr(&self, va: VirtAddr)
        -> Result<PhysAddr, &'static str>
    {
        match self.walk(va) {
            Some(pte) => {
                if !pte.is_valid() {
                    Err("pte not valid")
                } else if !pte.is_user() {
                    Err("pte not mapped for user")
                } else {
                    Ok(pte.as_phys_addr())
                }
            }
            None => {
                Err("va not mapped")
            }
        }
    }

    /// # 功能说明
    /// 分配并初始化一个新的进程页表，
    /// 包含对陷阱处理跳板（trampoline）和进程陷阱帧（trapframe）内存区域的映射。  
    /// 该函数为新进程创建独立的页表页，设置必要的权限以支持用户态与内核态的切换。
    ///
    /// # 参数
    /// - `trapframe`: 传入陷阱帧的物理地址，表示进程上下文保存的物理页地址。
    ///
    /// # 返回值
    /// - `Some(Box<Self>)`：返回初始化好的页表盒装实例。  
    /// - `None`：分配页表或映射失败时返回 `None`。
    ///
    /// # 可能的错误
    /// - 页表页内存分配失败导致返回 `None`。  
    /// - `trampoline` 函数地址或 `trapframe` 物理地址转换失败会导致 `unwrap()` 触发 panic。  
    /// - 映射操作失败导致返回 `None`。
    ///
    /// # 安全性
    /// - 函数中使用了 `unsafe` 块初始化未初始化内存，需确保调用时环境安全。  
    /// - 调用者必须保证 `trapframe` 物理地址有效且正确。  
    /// - 返回的页表应妥善管理，避免内存泄漏或数据竞争。  
    /// - 适合在进程创建时调用，且需保证调用时无并发冲突。
    pub fn alloc_proc_pagetable(trapframe: usize) -> Option<Box<Self>> {
        extern "C" {
            fn trampoline();
        }

        let mut pagetable = unsafe { Box::<Self>::try_new_zeroed().ok()?.assume_init() };
        pagetable
            .map_pages(
                VirtAddr::from(TRAMPOLINE),
                PGSIZE,
                PhysAddr::try_from(trampoline as usize).unwrap(),
                PteFlag::R | PteFlag::X,
            )
            .ok()?;
        pagetable
            .map_pages(
                VirtAddr::from(TRAPFRAME),
                PGSIZE,
                PhysAddr::try_from(trapframe).unwrap(),
                PteFlag::R | PteFlag::W,
            )
            .ok()?;

        Some(pagetable)
    }

    /// # 功能说明
    /// 释放进程页表中与用户空间相关的虚拟内存映射，  
    /// 包括陷阱跳板（TRAMPOLINE）、陷阱帧（TRAPFRAME）以及进程用户空间占用的物理内存。  
    /// 该函数负责撤销映射并释放对应物理内存，通常在进程退出或页表销毁时调用。
    ///
    /// # 参数
    /// - `&mut self`：进程对应的页表的可变引用。  
    /// - `proc_size`：用户进程占用的虚拟地址空间大小（字节）。
    ///
    /// # 返回值
    /// 无返回值。
    ///
    /// # 可能的错误
    /// - 若 `proc_size` 不合法，可能导致释放区域错误。  
    /// - 释放过程中若映射关系异常，可能导致内存泄漏或访问异常。  
    /// - `uvm_unmap` 内部未显式处理错误，调用者需确保调用时环境有效。
    ///
    /// # 安全性
    /// - 调用时需保证页表和映射状态一致，避免并发访问冲突。  
    /// - 释放物理内存和虚拟映射属于危险操作，调用者需确保调用时无其他线程访问相关内存。  
    /// - 该函数安全接口，内部细节依赖 `uvm_unmap` 的正确实现。
    pub fn dealloc_proc_pagetable(&mut self, proc_size: usize) {
        self.uvm_unmap(TRAMPOLINE.into(), 1, false);
        self.uvm_unmap(TRAPFRAME.into(), 1, false);
        // free physical memory
        if proc_size > 0 {
            self.uvm_unmap(0, pg_round_up(proc_size)/PGSIZE, true);
        }
    }

    /// # 功能说明
    /// 初始化进程用户空间的第一个内存页（代码页），  
    /// 将传入的程序代码 `code` 拷贝到用户空间的固定起始位置（`USERTEXT`），  
    /// 并建立对应虚拟地址到物理页的映射，权限包括读、写、执行和用户访问。
    ///
    /// # 参数
    /// - `&mut self`：进程页表的可变引用。  
    /// - `code`：包含进程初始代码的字节切片。
    ///
    /// # 返回值
    /// 无返回值，初始化失败时通过 panic 终止。
    ///
    /// # 可能的错误
    /// - 当 `code` 长度超过一页大小（`PGSIZE`）时触发 panic。  
    /// - 页表映射失败时调用 `expect` 导致 panic。  
    /// - 内存拷贝过程中若指针无效可能导致未定义行为。
    ///
    /// # 安全性
    /// - 使用了 `unsafe` 创建零初始化的物理页内存，调用者需保证安全。  
    /// - 使用裸指针拷贝数据，调用时需保证目标内存有效且不重叠。  
    /// - 函数假设代码大小不会超过一页，否则会 panic。  
    /// - 适合在进程创建早期调用，需保证单线程环境或同步机制。
    pub fn uvm_init(&mut self, code: &[u8]) {
        if code.len() >= PGSIZE {
            panic!("initcode more than a page");
        }
 
        let mem = unsafe { RawSinglePage::new_zeroed() as *mut u8 };
        self.map_pages(
            VirtAddr::from(USERTEXT),
            PGSIZE,
            PhysAddr::try_from(mem as usize).unwrap(),
            PteFlag::R | PteFlag::W | PteFlag::X | PteFlag::U)
            .expect("map_page error");
        unsafe { ptr::copy_nonoverlapping(code.as_ptr(), mem, code.len()); }
    }

    /// # 功能说明
    /// 为进程用户空间从 `old_size` 扩展到 `new_size` 分配新的内存页，
    /// 并建立对应的虚拟地址到物理页的映射，权限包括读、写、执行和用户访问。  
    /// 如果 `new_size` 小于或等于 `old_size`，则不做任何操作直接返回。  
    /// 该函数负责为进程用户空间分配新的页并初始化映射，支持动态扩展内存。
    ///
    /// # 参数
    /// - `&mut self`：进程页表的可变引用。  
    /// - `old_size`：当前用户空间大小（字节）。  
    /// - `new_size`：期望扩展后的用户空间大小（字节）。
    ///
    /// # 返回值
    /// - `Ok(usize)`：返回实际分配后的用户空间大小（字节）。  
    /// - `Err(())`：分配或映射失败时返回错误。
    ///
    /// # 可能的错误
    /// - 新物理页分配失败导致返回错误。  
    /// - 映射新页失败导致返回错误，并回滚已分配的页。  
    /// - 参数 `new_size` 非法（小于 `old_size`）时不报错但不分配。
    ///
    /// # 安全性
    /// - 函数内部使用了 `unsafe` 代码调用裸指针相关方法，需确保内存分配和映射安全。  
    /// - 回滚机制确保失败时不泄漏物理页内存。  
    /// - 函数假设调用环境为单线程或已做好同步，防止并发访问冲突。  
    /// - 返回的用户空间大小为页对齐值，调用者需留意对齐细节。
    pub fn uvm_alloc(&mut self, old_size: usize, new_size: usize) -> Result<usize, ()> {
        if new_size <= old_size {
            return Ok(old_size)
        }

        let old_size = pg_round_up(old_size);
        for cur_size in (old_size..new_size).step_by(PGSIZE) {
            match unsafe { RawSinglePage::try_new_zeroed() } {
                Err(_) => {
                    self.uvm_dealloc(cur_size, old_size);
                    return Err(())
                },
                Ok(mem) => {
                    match self.map_pages(
                        unsafe { VirtAddr::from_raw(cur_size) },
                        PGSIZE, 
                        unsafe { PhysAddr::from_raw(mem as usize) }, 
                        PteFlag::R | PteFlag::W | PteFlag::X | PteFlag::U
                    ) {
                        Err(s) => {
                            #[cfg(feature = "kernel_warning")]
                            println!("kernel warning: uvm_alloc occurs {}", s);
                            unsafe { RawSinglePage::from_raw_and_drop(mem); }
                            self.uvm_dealloc(cur_size, old_size);
                            return Err(())
                        },
                        Ok(_) => {
                            // the mem raw pointer is leaked
                            // but recorded in the pagetable at virtual address cur_size
                        },
                    }
                },
            }
        }

        Ok(new_size)
    }

    /// # 功能说明
    /// 释放用户空间中从 `new_size` 到 `old_size` 之间的内存页，  
    /// 通过减少线性大小实现用户地址空间的收缩。  
    /// 该函数对内存大小进行页对齐后，调用底层解除映射和释放物理内存。
    ///
    /// # 参数
    /// - `&mut self`：进程页表的可变引用。  
    /// - `old_size`：当前用户空间大小（字节）。  
    /// - `new_size`：期望释放后的用户空间大小（字节）。
    ///
    /// # 返回值
    /// 返回实际调整后的用户空间大小（即 `new_size`）。
    ///
    /// # 可能的错误
    /// - 当 `new_size` 大于等于 `old_size` 时，不进行释放，直接返回。  
    /// - 解除映射失败时没有显式返回错误，调用者需确保调用环境合法。
    ///
    /// # 安全性
    /// - 该函数安全接口，内部调用的 `uvm_unmap` 负责内存释放和映射解除。  
    /// - 调用时需保证页表结构完整且无并发修改，避免内存访问冲突。  
    /// - 释放操作应在单线程环境或已同步环境下执行。
    pub fn uvm_dealloc(&mut self, old_size: usize, new_size: usize) -> usize {
        if new_size >= old_size {
            return old_size
        }

        let old_size_aligned = pg_round_up(old_size);
        let new_size_aligned = pg_round_up(new_size);
        if new_size_aligned < old_size_aligned {
            let count = (old_size_aligned - new_size_aligned) / PGSIZE;
            self.uvm_unmap(new_size_aligned, count, true);
        }

        new_size
    }

    /// # 功能说明
    /// 解除从虚拟地址 `va` 开始连续 `count` 页的映射，
    /// 可选择是否释放对应的物理内存页。  
    /// 该函数用于回收进程用户空间的内存映射及物理页资源。
    ///
    /// # 参数
    /// - `&mut self`：进程页表的可变引用。  
    /// - `va`：起始虚拟地址，必须页对齐。  
    /// - `count`：需要解除映射的页数。  
    /// - `freeing`：布尔标志，若为 `true`，释放对应的物理页内存。
    ///
    /// # 返回值
    /// 无返回值。
    ///
    /// # 可能的错误
    /// - `va` 非页对齐时触发 panic。  
    /// - 找不到对应虚拟地址的页表项时触发 panic。  
    /// - 页表项无效或非叶子页表项时触发 panic。
    ///
    /// # 安全性
    /// - 使用了 `unsafe` 代码释放裸指针指向的物理页内存，调用者需确保内存安全。  
    /// - 解除映射和释放操作需在单线程或同步环境下执行，避免竞态条件。  
    /// - 解除映射后页表项会清零，防止悬挂指针访问。
    pub fn uvm_unmap(&mut self, va: usize, count: usize, freeing: bool) {
        if va % PGSIZE != 0 {
            panic!("va not page aligned");
        }

        for ca in (va..(va+PGSIZE*count)).step_by(PGSIZE) {
            let pte = self.walk_mut(unsafe {VirtAddr::from_raw(ca)})
                                        .expect("unable to find va available");
            if !pte.is_valid() {
                panic!("this pte is not valid");
            }
            if !pte.is_leaf() {
                panic!("this pte is not a leaf");
            }
            if freeing {
                let pa = pte.as_phys_addr();
                unsafe { RawSinglePage::from_raw_and_drop(pa.into_raw() as *mut u8); }
            }
            pte.write_zero();
        }
    }

    /// # 功能说明
    /// 将指定虚拟地址 `va` 对应的页表项标记为对用户态无效，  
    /// 通常用于设置用户空间的保护页（guard page），防止用户程序访问。
    ///
    /// # 参数
    /// - `&mut self`：页表的可变引用。  
    /// - `va`：需要标记的虚拟地址。
    ///
    /// # 返回值
    /// 无返回值。
    ///
    /// # 可能的错误
    /// - 若虚拟地址 `va` 对应的页表项不存在，函数会 panic。  
    /// - `VirtAddr::try_from(va)` 转换失败时触发 panic。
    ///
    /// # 安全性
    /// - 函数修改页表项权限，调用时需确保虚拟地址合法且页表状态一致。  
    /// - 该操作影响用户态访问权限，错误使用可能导致用户程序异常或安全问题。
    pub fn uvm_clear(&mut self, va: usize) {
        let pte = self.walk_mut(VirtAddr::try_from(va).unwrap())
                                                .expect("cannot find available pte");
        pte.clear_user();
    }

    /// # 功能说明
    /// 复制当前页表所管理的用户空间内存到子进程的页表 `child_pgt`，  
    /// 实现用户空间内存的逐页深拷贝，常用于进程创建（fork）时的地址空间复制。  
    /// 对每个页表项执行内存页克隆，建立子页表对应的映射关系，权限保持一致。
    ///
    /// # 参数
    /// - `&mut self`：当前（父）进程的页表可变引用。  
    /// - `child_pgt`：子进程的页表可变引用。  
    /// - `size`：需复制的用户空间大小（字节）。
    ///
    /// # 返回值
    /// - `Ok(())`：复制成功。  
    /// - `Err(())`：复制过程中出现错误，且已回滚部分已映射的页。
    ///
    /// # 可能的错误
    /// - 当前页表项不存在时触发 panic（`expect("pte not exist")`）。  
    /// - 物理页克隆失败或子页表映射失败时，函数会清理已映射的页并返回错误。  
    /// - 内存分配失败导致 `try_clone()` 返回错误。
    ///
    /// # 安全性
    /// - 函数中多处使用 `unsafe` 代码，调用裸指针操作内存，调用者需保证上下文安全。  
    /// - 回滚机制确保部分失败时释放已分配内存，避免泄漏。  
    /// - 函数假设调用时页表状态一致，且无并发访问，调用者需保证同步。
    pub fn uvm_copy(&mut self, child_pgt: &mut Self, size: usize) -> Result<(), ()> {
        for i in (0..size).step_by(PGSIZE) {
            let va = unsafe { VirtAddr::from_raw(i) };
            let pte = self.walk(va).expect("pte not exist");
            let mem = unsafe { pte.try_clone() };
            if let Ok(mem) = mem {
                let perm = pte.read_perm();
                if child_pgt.map_pages(va, PGSIZE,
                    unsafe { PhysAddr::from_raw(mem as usize) }, perm).is_ok()
                {
                    continue
                }
                unsafe { RawSinglePage::from_raw_and_drop(mem); }
            }
            child_pgt.uvm_unmap(0, i/PGSIZE, true);
            return Err(())
        }
        Ok(())
    }

    /// # 功能说明
    /// 从用户虚拟地址 `srcva` 处开始，复制一个以空字符 (`0`) 结尾的字符串到内核缓冲区 `dst` 中。  
    /// 复制过程逐页访问，自动处理页边界，直到遇到字符串结束符或目标缓冲区满。  
    /// 该函数用于从用户空间安全地读取字符串数据。
    ///
    /// # 参数
    /// - `&self`：当前页表的不可变引用。  
    /// - `srcva`：用户空间字符串起始虚拟地址。  
    /// - `dst`：内核中用于存放复制字符串的可变字节切片。
    ///
    /// # 返回值
    /// - `Ok(())`：成功复制字符串（遇到空字符结尾）。  
    /// - `Err(&'static str)`：目标缓冲区空间不足以完整复制字符串时返回错误。
    ///
    /// # 可能的错误
    /// - `srcva` 非法或未映射导致虚拟地址转换失败，返回错误。  
    /// - 目标缓冲区长度不足，导致未找到字符串结束符时返回错误。
    ///
    /// # 安全性
    /// - 函数内部使用了大量 `unsafe` 操作裸指针读取内存，调用时需确保页表映射正确且内存有效。  
    /// - 访问用户虚拟地址时，需防止越界和非法访问，避免内核崩溃。  
    /// - 该函数为只读操作，不修改用户内存，调用时线程安全。
    pub fn copy_in_str(&self, srcva: usize, dst: &mut [u8])
        -> Result<(), &'static str>
    {
        let mut i: usize = 0;
        let mut va = VirtAddr::try_from(srcva)?;

        // iterate through the raw content page by page
        while i < dst.len() {
            let mut base = va;
            base.pg_round_down();
            let distance = (va - base).as_usize();
            let mut pa_ptr = unsafe {
                self.walk_addr(base)?
                    .as_ptr()
                    .offset(distance as isize)
            };
            let mut va_ptr = va.as_ptr();
            
            // iterate througn each u8 in a page
            let mut count = min(PGSIZE - distance, dst.len() - i);
            while count > 0 {
                unsafe {
                    dst[i] = ptr::read(pa_ptr);
                    if dst[i] == 0 {
                        return Ok(())
                    }
                    i += 1;
                    count -= 1;
                    pa_ptr = pa_ptr.add(1);
                    va_ptr = va_ptr.add(1);
                }
            }

            base.add_page();
            va = base;
        }

        Err("copy_in_str: dst not enough space")
    }

    /// # 功能说明
    /// 将内核中的数据从指针 `src` 指向的缓冲区复制到用户虚拟地址空间中的目标地址 `dst`，
    /// 复制长度为 `count` 字节。该函数会自动处理跨页边界的拷贝，  
    /// 并确保目标用户地址可写且映射有效。
    ///
    /// # 参数
    /// - `&mut self`：页表的可变引用。  
    /// - `src`：源数据的内核空间指针。  
    /// - `dst`：目标用户虚拟地址。  
    /// - `count`：需要复制的字节数。
    ///
    /// # 返回值
    /// - `Ok(())`：数据成功复制。  
    /// - `Err(())`：复制失败，通常因目标用户地址无效或不可写。
    ///
    /// # 可能的错误
    /// - 当 `count` 为 0 时，直接返回成功。  
    /// - 目标虚拟地址转换为物理地址失败时返回错误。  
    /// - 跨页复制时如遇无效页表映射也会返回错误。
    ///
    /// # 安全性
    /// - 使用了大量 `unsafe` 代码访问裸指针，调用者需保证源地址有效且目标内存可写。  
    /// - 目标地址必须是合法且映射的用户空间地址，否则可能引发内存安全问题。  
    /// - 该函数操作涉及内核与用户空间交互，调用时需确保上下文安全及同步。
    pub fn copy_out(&mut self, mut src: *const u8, mut dst: usize, mut count: usize)
        -> Result<(), ()>
    {
        if count == 0 {
            return Ok(())
        }

        let mut va = VirtAddr::try_from(dst).map_err(|_| ())?;
        va.pg_round_down();
        loop {
            let mut pa;
            match self.walk_addr_mut(va) {
                Ok(phys_addr) => pa = phys_addr,
                Err(s) => {
                    #[cfg(feature = "kernel_warning")]
                    println!("kernel warning: {} when pagetable copy_out", s);
                    return Err(())
                }
            }
            let off = dst - va.as_usize();
            let off_from_end = PGSIZE - off;
            let off = off as isize;
            let dst_ptr = unsafe { pa.as_mut_ptr().offset(off) };
            if off_from_end > count {
                unsafe { ptr::copy(src, dst_ptr, count); }
                return Ok(())
            }
            unsafe { ptr::copy(src, dst_ptr, off_from_end); }
            count -= off_from_end;
            src = unsafe { src.offset(off_from_end as isize) };
            dst += off_from_end;
            va.add_page();
            debug_assert_eq!(dst, va.as_usize());
        }
    }

    /// # 功能说明
    /// 从用户虚拟地址 `src` 处复制 `count` 字节数据到内核空间的缓冲区 `dst`。  
    /// 函数会自动处理跨页边界的数据复制，确保访问的用户地址映射有效。  
    /// 该函数用于安全地从用户空间读取数据到内核空间。
    ///
    /// # 参数
    /// - `&self`：当前页表的不可变引用。  
    /// - `src`：用户空间源虚拟地址。  
    /// - `dst`：内核空间目标缓冲区的裸指针。  
    /// - `count`：需要复制的字节数。
    ///
    /// # 返回值
    /// - `Ok(())`：数据成功复制。  
    /// - `Err(())`：复制失败，通常因用户虚拟地址无效或未映射。
    ///
    /// # 可能的错误
    /// - 当 `count` 为 0 且起始虚拟地址不可访问时返回错误。  
    /// - 虚拟地址转换失败或访问权限不足时返回错误。  
    /// - 用户虚拟地址跨页但某页无效导致复制失败。
    ///
    /// # 安全性
    /// - 内部大量使用 `unsafe` 访问裸指针和用户内存，调用时需保证内存有效和映射正确。  
    /// - 调用者需保证 `dst` 指向有效内核内存且足够大以容纳复制内容。  
    /// - 函数不会修改用户空间数据，属于只读操作，调用时线程安全。
    pub fn copy_in(&self, mut src: usize, mut dst: *mut u8, mut count: usize)
        -> Result<(), ()>
    {
        let mut va = VirtAddr::try_from(src).unwrap();
        va.pg_round_down();

        if count == 0 {
            match self.walk_addr(va) {
                Ok(_) => return Ok(()),
                Err(s) => {
                    #[cfg(feature = "kernel_warning")]
                    println!("kernel warning: {} when pagetable copy_in", s);
                    return Err(())
                }
            }
        }

        loop {
            let pa;
            match self.walk_addr(va) {
                Ok(phys_addr) => pa = phys_addr,
                Err(s) => {
                    #[cfg(feature = "kernel_warning")]
                    println!("kernel warning: {} when pagetable copy_in", s);
                    return Err(())
                }
            }
            let off = src - va.as_usize();
            let off_from_end = PGSIZE - off;
            let off = off as isize;
            let src_ptr = unsafe { pa.as_ptr().offset(off) };
            if off_from_end > count {
                unsafe { ptr::copy(src_ptr, dst, count); }
                return Ok(())
            }
            unsafe { ptr::copy(src_ptr, dst, off_from_end); }
            count -= off_from_end;
            src += off_from_end;
            dst = unsafe { dst.offset(off_from_end as isize) };
            va.add_page();
            debug_assert_eq!(src, va.as_usize());
        }
    }
}

impl Drop for PageTable {
    /// # 功能说明
    /// 递归释放非顶级页表中所有页表项占用的页表页。  
    /// 该方法在 `PageTable` 被销毁时自动调用，清理子页表结构，  
    /// 但不负责释放物理内存（假设物理内存已被释放）。
    ///
    /// # 参数
    /// - `&mut self`：被销毁的页表的可变引用。
    ///
    /// # 返回值
    /// 无返回值。
    ///
    /// # 可能的错误
    /// - 如果页表项指向的子页表结构异常，可能导致未定义行为。
    ///
    /// # 安全性
    /// - 函数依赖页表项 `free()` 的正确实现安全释放子页表。  
    /// - 该自动调用函数不涉及并发控制，调用环境应保证线程安全。  
    /// - 假设物理内存已被其它机制释放，避免重复释放。
    fn drop(&mut self) {
        self.data.iter_mut().for_each(|pte| pte.free());
    }
}
