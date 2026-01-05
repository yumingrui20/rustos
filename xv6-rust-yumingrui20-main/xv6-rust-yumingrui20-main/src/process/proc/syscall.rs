//! 所有系统调用接口实现

use array_macro::array;

use alloc::string::String;
use alloc::boxed::Box;
use alloc::sync::Arc;
use core::convert::TryInto;
use core::fmt::Display;
use core::mem;

use crate::consts::{MAXPATH, MAXARG, MAXARGLEN, fs::MAX_DIR_SIZE};
use crate::process::PROC_MANAGER;
use crate::fs::{ICACHE, Inode, InodeType, LOG, File, Pipe, FileStat};
use crate::trap;

use super::{Proc, elf};

/// 系统调用结果类型
pub type SysResult = Result<usize, ()>;

/// 系统调用 trait 定义
///
/// 包含所有操作系统支持的系统调用方法，
/// 由 `Proc` 结构体实现具体功能。
pub trait Syscall {
    fn sys_fork(&mut self) -> SysResult;
    fn sys_exit(&mut self) -> SysResult;
    fn sys_wait(&mut self) -> SysResult;
    fn sys_pipe(&mut self) -> SysResult;
    fn sys_read(&mut self) -> SysResult;
    fn sys_kill(&mut self) -> SysResult;
    fn sys_exec(&mut self) -> SysResult;
    fn sys_fstat(&mut self) -> SysResult;
    fn sys_chdir(&mut self) -> SysResult;
    fn sys_dup(&mut self) -> SysResult;
    fn sys_getpid(&mut self) -> SysResult;
    fn sys_sbrk(&mut self) -> SysResult;
    fn sys_sleep(&mut self) -> SysResult;
    fn sys_uptime(&mut self) -> SysResult;
    fn sys_open(&mut self) -> SysResult;
    fn sys_write(&mut self) -> SysResult;
    fn sys_mknod(&mut self) -> SysResult;
    fn sys_unlink(&mut self) -> SysResult;
    fn sys_link(&mut self) -> SysResult;
    fn sys_mkdir(&mut self) -> SysResult;
    fn sys_close(&mut self) -> SysResult;
}

/// 为进程实现系统调用接口
impl Syscall for Proc {
    /// 创建当前进程的副本（子进程）
    ///
    /// # 功能说明
    /// 复制当前进程的状态（包括内存、文件描述符等）创建新进程。
    /// 子进程从 fork 返回点开始执行，与父进程共享内存直到写入时复制。
    ///
    /// # 返回值
    /// - 父进程：返回子进程 PID
    /// - 子进程：返回 0
    /// - 错误：返回 Err(())
    ///
    /// # 注意
    /// 实际实现委托给 `Proc::fork` 方法
    fn sys_fork(&mut self) -> SysResult {
        let ret = self.fork();

        #[cfg(feature = "trace_syscall")]
        println!("[{}].fork() = {:?}(pid)", self.excl.lock().pid, ret);

        ret
    }

    /// 终止当前进程
    ///
    /// # 功能说明
    /// 结束当前进程的执行，释放资源并通知父进程。
    /// 此函数不会返回，进程状态变为 ZOMBIE 直到父进程回收。
    ///
    /// # 参数
    /// - `exit_status`: 退出状态码（通过第一个参数获取）
    ///
    /// # 注意
    /// 调用后进程立即终止，不会返回到用户空间
    fn sys_exit(&mut self) -> SysResult {
        let exit_status = self.arg_i32(0);

        #[cfg(feature = "trace_syscall")]
        println!("[{}].exit(status={})", self.excl.lock().pid, exit_status);

        unsafe { PROC_MANAGER.exiting(self.index, exit_status); }
        unreachable!("process exit");
    }

    /// 等待子进程退出
    ///
    /// # 功能说明
    /// 挂起当前进程，直到任意子进程结束，然后回收子进程资源。
    ///
    /// # 参数
    /// - `status_addr`: 用户空间地址，用于存储子进程退出状态
    ///
    /// # 返回值
    /// - 成功：返回结束的子进程 PID
    /// - 错误：返回 Err(())
    fn sys_wait(&mut self) -> SysResult {
        let addr = self.arg_addr(0);
        let ret =  unsafe { PROC_MANAGER.waiting(self.index, addr) };

        #[cfg(feature = "trace_syscall")]
        println!("[{}].wait(addr={:#x}) = {:?}(pid)", self.excl.lock().pid, addr, ret);

        ret
    }

    /// 创建管道
    ///
    /// # 功能说明
    /// 创建一对相互连接的文件描述符，用于进程间通信。
    /// 一个描述符用于读取，另一个用于写入。
    ///
    /// # 参数
    /// - `pipefds_addr`: 用户空间地址，用于存储两个文件描述符
    ///
    /// # 返回值
    /// - 成功：返回 0
    /// - 错误：返回 Err(())
    ///
    /// # 流程
    /// 1. 分配两个文件描述符
    /// 2. 创建管道对象
    /// 3. 将描述符写入用户空间
    /// 4. 将管道对象绑定到文件描述符
    fn sys_pipe(&mut self) -> SysResult {
        let pipefds_addr = self.arg_addr(0);
        let addr_fdread = pipefds_addr;
        let addr_fdwrite = pipefds_addr+mem::size_of::<u32>();

        // 分配文件描述符
        let pdata = self.data.get_mut();
        let (fd_read, fd_write) = pdata.alloc_fd2().ok_or(())?;

        // 创建管道（返回读写文件对象）
        let (file_read, file_write) = Pipe::create().ok_or(())?;

        // 将描述符写入用户空间
        let fd_read_u32: u32 = fd_read.try_into().unwrap();
        let fd_write_u32: u32 = fd_write.try_into().unwrap();
        pdata.copy_out(&fd_read_u32 as *const u32 as *const u8, addr_fdread, mem::size_of::<u32>())?;
        pdata.copy_out(&fd_write_u32 as *const u32 as *const u8, addr_fdwrite, mem::size_of::<u32>())?;

        // 绑定文件对象到描述符
        pdata.open_files[fd_read].replace(file_read);
        pdata.open_files[fd_write].replace(file_write);

        #[cfg(feature = "trace_syscall")]
        println!("[{}].pipe(addr={:#x}) = ok, fd=[{},{}]", self.excl.lock().pid, pipefds_addr, fd_read, fd_write);

        Ok(0)
    }

    /// 从文件描述符读取数据
    ///
    /// # 功能说明
    /// 从指定文件描述符读取数据到用户空间缓冲区。
    ///
    /// # 参数
    /// - `fd`: 文件描述符
    /// - `user_addr`: 用户空间缓冲区地址
    /// - `count`: 要读取的字节数
    ///
    /// # 返回值
    /// - 成功：返回实际读取字节数
    /// - 错误：返回 Err(())
    ///
    /// # 安全
    /// 验证用户地址和计数有效性
    fn sys_read(&mut self) -> SysResult {
        let fd = self.arg_fd(0)?;
        let user_addr = self.arg_addr(1);
        let count = self.arg_i32(2);
        if count <= 0 || self.data.get_mut().check_user_addr(user_addr).is_err() {
            return Err(())
        }
        let count = count as u32;
        
        let file = self.data.get_mut().open_files[fd].as_ref().unwrap();
        let ret = file.fread(user_addr, count);

        #[cfg(feature = "trace_syscall")]
        println!("[{}].read(fd={}, addr={:#x}, count={}) = {:?}", self.excl.lock().pid, fd, user_addr, count, ret);

        ret.map(|count| count as usize)
    }

    /// 终止指定进程
    ///
    /// # 功能说明
    /// 向目标进程发送终止信号，使其退出执行。
    ///
    /// # 参数
    /// - `pid`: 目标进程ID
    ///
    /// # 返回值
    /// - 成功：返回 0
    /// - 错误：返回 Err(())
    ///
    /// # 注意
    /// 目前仅支持终止信号，不支持其他信号类型
    fn sys_kill(&mut self) -> SysResult {
        let pid = self.arg_i32(0);
        if pid < 0 {
            return Err(())
        }
        let pid = pid as usize;
        let ret = unsafe { PROC_MANAGER.kill(pid) };

        #[cfg(feature = "trace_syscall")]
        println!("[{}].kill(pid={}) = {:?}", self.excl.lock().pid, pid, ret);

        ret.map(|()| 0)
    }

    /// 执行新程序
    ///
    /// # 功能说明
    /// 替换当前进程的内存空间，加载并执行指定路径的ELF可执行文件。
    ///
    /// # 参数
    /// - `path`: 可执行文件路径
    /// - `argv`: 命令行参数数组
    ///
    /// # 返回值
    /// - 成功：不会返回（新程序开始执行）
    /// - 错误：返回 Err(())
    ///
    /// # 流程
    /// 1. 读取可执行文件路径
    /// 2. 读取命令行参数
    /// 3. 加载ELF文件
    /// 4. 设置新程序的初始状态
    fn sys_exec(&mut self) -> SysResult {
        let mut path: [u8; MAXPATH] = [0; MAXPATH];
        self.arg_str(0, &mut path).map_err(syscall_warning)?;

        let mut result: SysResult = Err(());
        let mut error = "too many arguments";
        let mut uarg: usize;
        let uargv = self.arg_addr(1);
        let mut argv: [Option<Box<[u8; MAXARGLEN]>>; MAXARG] = array![_ => None; MAXARG];
        for i in 0..MAXARG {
            // 获取第i个参数的地址
            match self.fetch_addr(uargv+i*mem::size_of::<usize>()) {
                Ok(addr) => uarg = addr,
                Err(s) => {
                    error = s;
                    break
                },
            }
            if uarg == 0 {
                match elf::load(self, &path, &argv[..i]) {
                    Ok(ret) => result = Ok(ret),
                    Err(s) => error = s,
                }
                break       
            }

            // 为参数分配内核空间
            match Box::try_new_zeroed() {
                Ok(b) => unsafe { argv[i] = Some(b.assume_init()) },
                Err(_) => {
                    error = "not enough kernel memory";
                    break
                },
            }

            // 将用户空间参数复制到内核
            if let Err(s) = self.fetch_str(uarg, argv[i].as_deref_mut().unwrap()) {
                error = s;
                break
            }
        }

        #[cfg(feature = "trace_syscall")]
        println!("[{}].exec({}, {:#x}) = {:?}", self.excl.lock().pid, String::from_utf8_lossy(&path), uargv, result);

        if result.is_err() {
            syscall_warning(error);
        }
        result
    }

    /// 获取文件状态信息
    ///
    /// # 功能说明
    /// 获取指定文件描述符的状态信息并复制到用户空间。
    ///
    /// # 参数
    /// - `fd`: 文件描述符
    /// - `addr`: 用户空间地址（用于存储FileStat结构）
    ///
    /// # 返回值
    /// - 成功：返回 0
    /// - 错误：返回 Err(())
    fn sys_fstat(&mut self) -> SysResult {
        let fd = self.arg_fd(0)?;
        let addr = self.arg_addr(1);
        let mut stat = FileStat::uninit();
        let file = self.data.get_mut().open_files[fd].as_ref().unwrap();
        let ret = if file.fstat(&mut stat).is_err() {
            Err(())
        } else {
            let pgt = self.data.get_mut().pagetable.as_mut().unwrap();
            if pgt.copy_out(&stat as *const FileStat as *const u8, addr, mem::size_of::<FileStat>()).is_err() {
                Err(())
            } else {
                Ok(0)
            }
        };

        #[cfg(feature = "trace_syscall")]
        println!("[{}].fstat(fd={}, addr={:#x}) = {:?}", self.excl.lock().pid, fd, addr, stat);

        ret
    }

    /// 更改当前工作目录
    ///
    /// # 功能说明
    /// 将当前进程的工作目录更改为指定路径。
    ///
    /// # 参数
    /// - `path`: 目标目录路径
    ///
    /// # 返回值
    /// - 成功：返回 0
    /// - 错误：返回 Err(())
    ///
    /// # 流程
    /// 1. 验证路径存在且是目录
    /// 2. 更新进程的当前工作目录
    fn sys_chdir(&mut self) -> SysResult {
        let mut path: [u8; MAXPATH] = [0; MAXPATH];
        self.arg_str(0, &mut path).map_err(syscall_warning)?;

        LOG.begin_op();
        let inode: Inode;
        if let Some(i) = ICACHE.namei(&path) {
            inode = i;
        } else {
            LOG.end_op();
            return Err(())
        }
        let idata = inode.lock();
        if idata.get_itype() != InodeType::Directory {
            drop(idata); drop(inode); LOG.end_op();
            return Err(())
        }
        drop(idata);
        let old_cwd = self.data.get_mut().cwd.replace(inode);
        debug_assert!(old_cwd.is_some());
        drop(old_cwd);
        LOG.end_op();
        Ok(0)
    }

    /// 复制文件描述符
    ///
    /// # 功能说明
    /// 创建指定文件描述符的副本，指向相同的文件对象。
    ///
    /// # 参数
    /// - `old_fd`: 原文件描述符
    ///
    /// # 返回值
    /// - 成功：返回新文件描述符
    /// - 错误：返回 Err(())
    fn sys_dup(&mut self) -> SysResult {
        let old_fd = self.arg_fd(0)?;
        let pd = self.data.get_mut();
        let new_fd = pd.alloc_fd().ok_or(())?;
        
        let old_file = pd.open_files[old_fd].as_ref().unwrap();
        let new_file = Arc::clone(old_file);
        let none_file = pd.open_files[new_fd].replace(new_file);
        debug_assert!(none_file.is_none());

        #[cfg(feature = "trace_syscall")]
        println!("[{}].dup({}) = {}(fd)", self.excl.lock().pid, old_fd, new_fd);

        Ok(new_fd)
    }

    /// 获取当前进程ID
    ///
    /// # 功能说明
    /// 返回当前进程的唯一标识符（PID）。
    ///
    /// # 返回值
    /// 当前进程的PID
    fn sys_getpid(&mut self) -> SysResult {
        let pid = self.excl.lock().pid;

        #[cfg(feature = "trace_syscall")]
        println!("[{}].getpid() = {}", pid, pid);

        Ok(pid)
    }

    /// 调整进程堆大小
    ///
    /// # 功能说明
    /// 增加或减少进程的堆内存空间。
    ///
    /// # 参数
    /// - `increment`: 堆大小的增量（字节数）
    ///
    /// # 返回值
    /// - 成功：返回原堆顶地址
    /// - 错误：返回 Err(())
    ///
    /// # 注意
    /// 实际实现委托给 `ProcData::sbrk` 方法
    fn sys_sbrk(&mut self) -> SysResult {
        let increment = self.arg_i32(0);
        let ret = self.data.get_mut().sbrk(increment);

        #[cfg(feature = "trace_syscall")]
        println!("[{}].sbrk({}) = {:?}", self.excl.lock().pid, increment, ret);

        ret
    }

    /// 使进程休眠
    ///
    /// # 功能说明
    /// 使当前进程休眠指定数量的时钟周期。
    ///
    /// # 参数
    /// - `count`: 要休眠的时钟周期数
    ///
    /// # 返回值
    /// - 成功：返回 0
    /// - 错误：返回 Err(())
    ///
    /// # 注意
    /// 实际实现委托给 `trap::clock_sleep`
    fn sys_sleep(&mut self) -> SysResult {
        let count = self.arg_i32(0);
        if count < 0 {
            return Err(())
        }
        let count = count as usize;
        let ret = trap::clock_sleep(self, count);

        #[cfg(feature = "trace_syscall")]
        println!("[{}].sleep({}) = {:?}", self.excl.lock().pid, count, ret);

        ret.map(|()| 0)
    }

    /// 获取系统运行时间
    ///
    /// # 功能说明
    /// 返回系统启动以来的时钟周期数（近似系统运行时间）。
    ///
    /// # 返回值
    /// 系统时钟周期数
    ///
    /// # 注意
    /// 实际实现委托给 `trap::clock_read`
    fn sys_uptime(&mut self) -> SysResult {
        let ret = trap::clock_read();

        #[cfg(feature = "trace_syscall")]
        println!("[{}].uptime() = {}", self.excl.lock().pid, ret);

        Ok(ret)
    }

    /// 打开或创建文件
    ///
    /// # 功能说明
    /// 打开指定路径的文件，返回关联的文件描述符。
    ///
    /// # 参数
    /// - `path`: 文件路径
    /// - `flags`: 打开标志（目前未完全实现）
    ///
    /// # 返回值
    /// - 成功：返回文件描述符
    /// - 错误：返回 Err(())
    ///
    /// # 注意
    /// 创建特殊文件应使用 `sys_mknod`
    fn sys_open(&mut self) -> SysResult {
        let mut path: [u8; MAXPATH] = [0; MAXPATH];
        self.arg_str(0, &mut path).map_err(syscall_warning)?;
        let flags = self.arg_i32(1);
        if flags < 0 {
            return Err(())
        }

        let fd = self.data.get_mut().alloc_fd().ok_or(())?;
        let file = File::open(&path, flags).ok_or(())?;
        let none_file = self.data.get_mut().open_files[fd].replace(file);
        debug_assert!(none_file.is_none());

        #[cfg(feature = "trace_syscall")]
        println!("[{}].open({}, {:#x}) = {}(fd)", self.excl.lock().pid, String::from_utf8_lossy(&path), flags, fd);

        Ok(fd)
    }

    /// 写入文件描述符
    ///
    /// # 功能说明
    /// 将用户空间缓冲区的数据写入指定文件描述符。
    ///
    /// # 参数
    /// - `fd`: 文件描述符
    /// - `user_addr`: 用户空间数据地址
    /// - `count`: 要写入的字节数
    ///
    /// # 返回值
    /// - 成功：返回实际写入字节数
    /// - 错误：返回 Err(())
    ///
    /// # 安全
    /// 验证用户地址和计数有效性
    fn sys_write(&mut self) -> SysResult {
        let fd = self.arg_fd(0)?;
        let user_addr = self.arg_addr(1);
        let count = self.arg_i32(2);
        if count <= 0 || self.data.get_mut().check_user_addr(user_addr).is_err() {
            return Err(())
        }
        let count = count as u32;

        let file = self.data.get_mut().open_files[fd].as_ref().unwrap();
        let ret = file.fwrite(user_addr, count);

        #[cfg(feature = "trace_syscall")]
        println!("[{}].write({}, {:#x}, {}) = {:?}", self.excl.lock().pid, fd, user_addr, count, ret);

        ret.map(|count| count as usize)
    }

    /// 创建设备文件
    ///
    /// # 功能说明
    /// 在文件系统中创建设备特殊文件。
    ///
    /// # 参数
    /// - `path`: 文件路径
    /// - `major`: 主设备号
    /// - `minor`: 次设备号
    ///
    /// # 返回值
    /// - 成功：返回 0
    /// - 错误：返回 Err(())
    fn sys_mknod(&mut self) -> SysResult {
        let mut path: [u8; MAXPATH] = [0; MAXPATH];
        self.arg_str(0, &mut path).map_err(syscall_warning)?;
        let major = self.arg_i32(1);
        let minor = self.arg_i32(2);
        if major < 0 || minor < 0 {
            return Err(())
        }

        let major: u16 = major.try_into().map_err(|_| ())?;
        let minor: u16 = minor.try_into().map_err(|_| ())?;
        LOG.begin_op();
        let ret = ICACHE.create(&path, InodeType::Device, major, minor, true).ok_or(());

        #[cfg(feature = "trace_syscall")]
        println!("[{}].mknod(path={}, major={}, minor={}) = {:?}",
            self.excl.lock().pid, String::from_utf8_lossy(&path), major, minor, ret);

        let ret = ret.map(|inode| {drop(inode);0});
        LOG.end_op();
        ret
    }

    /// 删除文件链接
    ///
    /// # 功能说明
    /// 删除文件路径的链接，减少文件的链接计数。
    /// 如果链接计数降为0且没有进程打开该文件，则释放文件资源。
    ///
    /// # 参数
    /// - `path`: 要删除的文件路径
    ///
    /// # 返回值
    /// - 成功：返回 0
    /// - 错误：返回 Err(())
    fn sys_unlink(&mut self) -> SysResult {
        let mut path: [u8; MAXPATH] = [0; MAXPATH];
        self.arg_str(0, &mut path).map_err(syscall_warning)?;

        LOG.begin_op();
        let mut name: [u8; MAX_DIR_SIZE] = [0; MAX_DIR_SIZE];
        let dir_inode: Inode;
        if let Some(inode) = ICACHE.namei_parent(&path, &mut name) {
            dir_inode = inode;
        } else {
            LOG.end_op();
            return Err(())
        }

        let mut dir_idata = dir_inode.lock();
        let ret = dir_idata.dir_unlink(&name);
        drop(dir_idata);
        drop(dir_inode);
        LOG.end_op();

        #[cfg(feature = "trace_syscall")]
        println!("[{}].unlink(path={}) = {:?}", self.excl.lock().pid, String::from_utf8_lossy(&path), ret);

        ret.map(|()| 0)
    }

    /// 创建文件硬链接
    ///
    /// # 功能说明
    /// 为现有文件创建新的硬链接（路径别名）。
    ///
    /// # 参数
    /// - `old_path`: 现有文件路径
    /// - `new_path`: 新链接路径
    ///
    /// # 返回值
    /// - 成功：返回 0
    /// - 错误：返回 Err(())
    ///
    /// # 流程
    /// 1. 查找原文件
    /// 2. 增加原文件链接计数
    /// 3. 在新路径创建链接
    fn sys_link(&mut self) -> SysResult {
        let mut old_path: [u8; MAXPATH] = [0; MAXPATH];
        let mut new_path: [u8; MAXPATH] = [0; MAXPATH];
        self.arg_str(0, &mut old_path).map_err(syscall_warning)?;
        self.arg_str(1, &mut new_path).map_err(syscall_warning)?;

        LOG.begin_op();

        // 查找原文件
        let old_inode = ICACHE.namei(&old_path).ok_or_else(|| {LOG.end_op(); ()})?;
        let mut old_idata = old_inode.lock();
        let (old_dev, old_inum) = old_idata.get_dev_inum();
        if old_idata.get_itype() == InodeType::Directory {
            syscall_warning("trying to create new link to a directory");
            LOG.end_op();
            return Err(())
        }
        old_idata.link();
        old_idata.update();
        drop(old_idata);

        // 如果无法创建新路径
        let revert_link = move |inode: Inode| {
            let mut idata = inode.lock();
            idata.unlink();
            idata.update();
            drop(idata);
            drop(inode);
            LOG.end_op();
        };

        // 创建新路径
        let mut name: [u8; MAX_DIR_SIZE] = [0; MAX_DIR_SIZE];
        let new_inode: Inode;
        match ICACHE.namei_parent(&new_path, &mut name) {
            Some(inode) => new_inode = inode,
            None => {
                revert_link(old_inode);
                return Err(())
            }
        }
        let mut new_idata = new_inode.lock();
        if new_idata.get_dev_inum().0 != old_dev || new_idata.dir_link(&name, old_inum).is_err() {
            revert_link(old_inode);
            return Err(())
        }
        drop(new_idata);
        drop(new_inode);
        drop(old_inode);

        LOG.end_op();

        #[cfg(feature = "trace_syscall")]
        println!("[{}].link(old_path={}, new_path={})", self.excl.lock().pid,
            String::from_utf8_lossy(&old_path), String::from_utf8_lossy(&new_path));
        
        Ok(0)
    }

    /// 创建目录
    ///
    /// # 功能说明
    /// 在指定路径创建新目录。
    ///
    /// # 参数
    /// - `path`: 目录路径
    ///
    /// # 返回值
    /// - 成功：返回 0
    /// - 错误：返回 Err(())
    ///
    /// # 注意
    /// 目录权限模式尚未实现
    fn sys_mkdir(&mut self) -> SysResult {
        let mut path: [u8; MAXPATH] = [0; MAXPATH];
        self.arg_str(0, &mut path).map_err(syscall_warning)?;

        LOG.begin_op();
        let ret = ICACHE.create(&path, InodeType::Directory, 0, 0, false);

        #[cfg(feature = "trace_syscall")]
        println!("[{}].mkdir(path={}) = {:?}", self.excl.lock().pid, String::from_utf8_lossy(&path), ret);

        let ret = match ret {
            Some(inode) => {
                drop(inode);
                Ok(0)
            },
            None => Err(()),
        };
        LOG.end_op();
        ret
    }

    /// 关闭文件描述符
    ///
    /// # 功能说明
    /// 关闭指定文件描述符，释放相关资源。
    ///
    /// # 参数
    /// - `fd`: 要关闭的文件描述符
    ///
    /// # 返回值
    /// - 成功：返回 0
    /// - 错误：返回 Err(())
    fn sys_close(&mut self) -> SysResult {
        let fd = self.arg_fd(0)?;
        let file = self.data.get_mut().open_files[fd].take();

        #[cfg(feature = "trace_syscall")]
        println!("[{}].close(fd={}), file={:?}", self.excl.lock().pid, fd, file);

        drop(file);
        Ok(0)
    }
}

/// 系统调用警告函数
///
/// # 功能说明
/// 输出系统调用相关的警告信息，用于调试。
///
/// # 参数
/// - `s`: 警告信息（实现Display trait）
///
/// # 注意
/// 仅在启用 `kernel_warning` 特性时实际输出
#[inline]
fn syscall_warning<T: Display>(s: T) {
    #[cfg(feature = "kernel_warning")]
    println!("syscall waring: {}", s);
}
