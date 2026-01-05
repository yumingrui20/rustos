[![Review Assignment Due Date](https://classroom.github.com/assets/deadline-readme-button-22041afd0340ce965d47ae6ef1cefeee28c7c493a6346c4f15d667ab976d596c.svg)](https://classroom.github.com/a/XbQLAwmK)
# xv6-rust 代码阅读指南

根据课程要求，需要各位同学阅读三部分的代码：

- 系统启动：`start.rs`，`rmain.rs`
- 内存管理：mm 目录下的各个文件
- 进程管理：：process 目录下的各个文件
  - 中断处理部分位于`trap.rs`中

本文档提供xv6-rust项目代码阅读与运行的指导，供各位参考

## 代码结构

```
├── asm
│   ├── entry.S            // 汇编启动入口代码，设置堆栈并跳转到内核主函数
│   ├── init.c             // 用于生成用户态初始化代码的C语言源文件
│   ├── initcode.S         // 用户初始化程序(init)的汇编代码，嵌入到内核镜像中
│   ├── kernelvec.S        // 内核中断向量表定义，处理中断和异常入口
│   ├── swtch.S            // 上下文切换的汇编实现，保存/恢复CPU状态
│   └── trampoline.S       // 用户态与内核态切换的跳板页代码，含sret指令
├── consts
│   ├── driver.rs          // 驱动模块所用常量，例如设备寄存器基址等
│   ├── fs.rs              // 文件系统相关常量定义，如磁盘块大小等
│   ├── memlayout.rs       // 内核虚拟地址空间布局与物理内存布局定义
│   ├── mod.rs             // consts模块总入口，统一导出子模块
│   ├── param.rs           // 全局系统参数，例如最大进程数、内核栈大小
│   └── riscv.rs           // RISC-V 架构相关常量，例如CSR寄存器号等
├── driver
│   ├── console.rs         // 控制台输出驱动，依赖UART输出
│   ├── mod.rs             // driver模块入口，统一导出各设备驱动
│   ├── uart.rs            // UART串口驱动，实现字符收发
│   └── virtio_disk.rs     // VirtIO磁盘驱动，处理块设备的读写操作
├── fs
│   ├── bio.rs             // 块缓冲区管理，缓存磁盘块读写
│   ├── block.rs           // 磁盘块分配器，管理空闲块
│   ├── file
│   │   ├── mod.rs         // file模块入口，统一导出文件与管道接口
│   │   └── pipe.rs        // 管道文件实现，实现无名管道的读写
│   ├── inode.rs           // 索引节点实现，表示文件的元数据
│   ├── log.rs             // 日志模块，支持原子文件系统操作
│   ├── mod.rs             // fs模块入口，统一导出文件系统各子模块
│   └── superblock.rs      // 超级块结构与加载逻辑，描述文件系统全局信息
├── ld
│   └── kernel.ld          // 链接脚本，控制内核各段的内存布局
├── lib.rs                 // 内核库模块入口，包含通用宏、panic等定义
├── main.rs                // 内核主函数，启动各子系统并进入第一个进程
├── mm
│   ├── addr.rs            // 虚拟地址与物理地址处理工具
│   ├── kalloc.rs          // 内核物理页分配器，实现简单的空闲页管理
│   ├── kvm.rs             // 内核页表初始化与映射操作
│   ├── list.rs            // 链表工具，支持双向链表实现
│   ├── mod.rs             // mm模块入口，统一导出内存管理模块
│   └── pagetable.rs       // 页表实现，包含页表项结构与映射函数
├── plic.rs                // PLIC 外部中断控制器驱动，实现中断使能与查询
├── printf.rs              // 内核 printf 实现，格式化字符串输出函数
├── process
│   ├── context.rs         // 上下文结构体，保存用户进程寄存器状态
│   ├── cpu.rs             // CPU状态与调度器实现，包含每核的调度器状态
│   ├── mod.rs             // process模块入口，导出进程调度与管理功能
│   ├── proc
│   │   ├── elf.rs         // ELF文件加载器，将用户程序加载进内存
│   │   ├── mod.rs         // proc子模块入口，导出进程创建、退出等功能
│   │   └── syscall.rs     // 系统调用处理器，提供用户态系统服务接口
│   └── trapframe.rs       // 陷入帧结构体，保存陷入内核时的用户态上下文
├── register
│   ├── clint.rs           // CLINT 定时中断控制器驱动，管理mtime中断
│   ├── mie.rs             // mie CSR 寄存器访问封装，控制中断使能位
│   ├── mod.rs             // register模块入口，导出CSR访问相关接口
│   ├── mstatus.rs         // mstatus CSR 操作，管理特权级状态
│   ├── satp.rs            // satp CSR 操作，管理页表根指针
│   ├── scause.rs          // scause CSR，分析异常或中断来源
│   ├── sie.rs             // sie CSR 操作，软件中断控制
│   ├── sip.rs             // sip CSR 状态寄存器封装，查询中断状态
│   └── sstatus.rs         // sstatus CSR 操作，管理S模式状态标志
├── rmain.rs               // 多核入口函数，初始化每个CPU的调度器与陷入向量
├── sleeplock.rs           // 睡眠锁实现，适合I/O等待的互斥保护
├── spinlock.rs            // 自旋锁实现，用于短时临界区互斥保护
├── start.rs               // 内核入口函数，启动第一个核的执行流程
└── trap.rs                // 中断与异常处理主逻辑，分派不同的trap类型
```

## xv6-rust 实验平台操作说明

本实验指导手册基于Rust语言开发的类xv6架构操作系统内核编写。下面介绍实验平台以及系统内核的操作方法，请根据说明进行模仿，确保执行的结果与说明一致。

### 实验平台操作方法

实验平台的操作通过Makefile实现，下面介绍本实验平台的Makefile允许执行的操作：

**`make qemu`**

- 该指令用于启动内核，执行用户程序与内核进行交互
- 执行该指令后，将会自动编译系统内核，用户程序，并生成初始化文件系统，最终启动QEMU
- 本指令不会检测到内核代码的修改，对内核代码进行修改后，请先执行`make clean`

正确执行该指令后应该看到如下输出

```
qemu-system-riscv64 -machine virt -bios none -kernel target/riscv64gc-unknown-none-elf/debug/xv6-rust -m 3G -smp 3 -nographic -drive file=fs.img,if=none,format=raw,id=x0 -device virtio-blk-device,drive=x0,bus=virtio-mmio-bus.0

xv6-rust is booting

KernelHeap: available physical memory [0x80054028, 0x88000000)
  buddy system: useful memory is 0x7fab000 bytes
  buddy system: leaf size is 16 bytes
  buddy system: free lists have 24 different sizes
  buddy system: alloc 0x300490 bytes meta data
  buddy system: 0x55000 bytes unavailable
KernelHeap: init memory done
hart 2 starting
hart 1 starting
file system: checking logs
file system: no need to recover
file system: setup done
init: starting sh
$ 
```

**`make qemu-gdb`**

- 该指令用于调试内核，首先启动QEMU并等待GDB连接，连接成功后按照GDB的指令运行内核
- 首先开启两个终端，在第一个终端中执行该指令，等待内核编译完成后启动QEMU

QEMU启动后输出如下

```
*** Now run 'gdb' in another window.
qemu-system-riscv64 -machine virt -bios none -kernel target/riscv64gc-unknown-none-elf/debug/xv6-rust -m 3G -smp 3 -nographic -drive file=fs.img,if=none,format=raw,id=x0 -device virtio-blk-device,drive=x0,bus=virtio-mmio-bus.0 -S -gdb tcp::26000
```

- 在第二个终端中执行`gdb-multiarch`（或任何可用的gdb），待gdb启动后执行`source .gdbinit`

此时GDB窗口输出如下

```
GNU gdb (Ubuntu 12.1-0ubuntu1~22.04.2) 12.1
Copyright (C) 2022 Free Software Foundation, Inc.
License GPLv3+: GNU GPL version 3 or later <http://gnu.org/licenses/gpl.html>
...
(gdb) source .gdbinit
The target architecture is set to "riscv:rv64".
warning: No executable has been specified and target does not support
determining executable automatically.  Try using the "file" command.
0x0000000000001000 in ?? ()
(gdb) 
```

至此，QEMU与GDB连接成功，可以在GDB中执行调试指令以运行内核，具体调试办法详见**内核调试方法**

**`make asm`**

- 该指令会执行objdump对内核进行反汇编，输出内核的汇编指令
- 输出结果在kernel.S文件中，其中主要包含函数名称，指令地址以及指令内容
- 可用于内核异常时根据异常指令地址定位出错函数

以系统启动位置的汇编代码为例，该文件结构如下

```assembly
0000000080000000 <_entry>:
    80000000:	0002c117          	auipc	sp,0x2c
    80000004:	00010113          	mv		sp,sp
    80000008:	6509                lui		a0,0x2
    8000000a:	f14025f3          	csrr	a1,mhartid
    8000000e:	0585                addi	a1,a1,1
    80000010:	02b50533          	mul		a0,a0,a1
    80000014:	912a                add		sp,sp,a0
    80000016:	00001097          	auipc	ra,0x1
    8000001a:	114080e7          	jalr	276(ra) # 8000112a <start>
```

**`make clean`**

- 该指令将清空所有编译结果，将实验环境初始化
- 包括用户程序编译结果，内核编译结果，文件系统镜像
- 同时还会清除`make asm`生成的反汇编结果

运行后将执行如下指令

```bash
rm -rf kernel.S
cargo clean
rm -f user/*.o user/*.d user/*.asm user/*.sym \
user/initcode user/initcode.out fs.img \
mkfs/mkfs .gdbinit xv6.out \
user/usys.S \
user/_cat user/_echo user/_forktest user/_grep user/_init user/_kill user/_ln user/_ls user/_mkdir user/_rm user/_sh user/_stressfs user/_usertests user/_grind user/_wc user/_zombie user/_sleep user/_pingpong user/_primes user/_find user/_xargs
```

------

### 系统内核操作方法

这里主要介绍两个常用快捷键

- `Ctrl+P`：输出内核中正在运行的进程列表
- `Ctrl+A+X`：停止内核运行，关闭QEMU

在执行`make qemu`后，系统内核会启动，并自动执行shell用户程序，该程序输出一个dollar符号并等待用户选择用户程序进行执行。

shell 是一个非常简洁的用户态程序，它为操作系统提供基本的用户交互接口。在内核中，它的源代码位于 `user/sh.c` 文件，是用户运行的第一个可交互程序。

**命令解析与执行**

- Shell 使用 `fork + exec` 模式执行用户输入的命令。

- 支持内建命令（如 `cd`）和外部程序（如 `ls`, `cat`, `sh`, `echo`）。

- 输入示例：

  ```
  $ echo hello
  $ ls
  ```

- 对于外部程序，shell 会：

  1. `fork` 创建子进程；
  2. 在子进程中调用 `exec` 替换为对应的用户程序；
  3. 父进程 `wait` 直到子进程结束。

**I/O 重定向（< 和 >）**

- 允许将输入或输出重定向到文件。

- 示例：

  ```bash
  $ echo hello > out.txt
  $ cat < out.txt
  ```

- 实现方式：

  - shell 在解析命令时检测 `<` 或 `>`；
  - 使用 `open()` 打开对应文件；
  - 使用 `dup()` 将标准输入（fd=0）或标准输出（fd=1）重定向到该文件描述符。

**管道支持（|）**

- 支持使用 `|` 连接多个命令的输出与输入。

- 示例：

  ```bash
  $ ls | grep usertests
  ```

- 实现方式：

  - shell 创建管道（`pipe(fd)`）；
  - fork 两个子进程，一个写管道，一个读管道；
  - 使用 `dup()` 将 stdout/stdin 重定向为管道端点。

**内建命令：**`cd`

- xv6 shell 支持内建的 `cd` 命令改变当前工作目录。

- 示例：

  ```bash
  $ cd /bin
  ```

- 特殊性：

  - `cd` 不能用 `exec` 实现，因为目录更改必须在当前进程（shell）中生效。
  - 因此，shell 检测到 `cd` 后直接调用 `chdir()` 而不 fork。

------

### 系统内核调试方法

**常用 GDB 调试命令**

1. 显示源码布局

```gdb
layout src
```

- 显示源代码窗口。
- 按 `Ctrl+L` 可刷新界面。
- 使用 `layout split` 可同时显示代码和汇编。

------

2. 设置断点

```gdb
b rust_main
b usertrap
b trap.rs:42
```

- 在指定函数或文件行号设置断点。
- 可以使用 `info break` 查看当前所有断点。
- 使用 `delete` 删除断点。

------

3. 运行与继续执行

```gdb
c        # continue，继续运行直到下一个断点
```

- 如果你设置了断点并执行 `continue`，GDB 会在命中断点时停止。

------

4. 单步调试

```gdb
s        # step：单步进入函数
n        # next：单步执行，不进入函数
fin      # finish：运行直到当前函数返回
```

- `s`（step）适合函数内部逐行观察。
- `n`（next）在调用函数时会跳过函数体。
- `fin`（finish）会跑到当前函数结束，常用于快速跳出。

------

5. 变量与表达式查看

```gdb
p var_name            # 打印变量值
p/x 0x80000000        # 以十六进制打印值
p *(int*)0x80000000   # 以 C 样式解引用地址
```

- `x/4x`：查看内存，十六进制模式。
- `x/s`：查看字符串内容。
- `display`：持续显示变量。
- `set var foo = 3`：更改变量值。

------

6. 查看调用栈与函数

```gdb
bt       # backtrace：查看调用栈
frame 1  # 切换到栈帧 1
info registers
```

- `bt` 是诊断内核 panic 和 trap 问题的重要工具。
- 可以在每一层 frame 中使用 `list`、`p` 查看局部变量。

------

7. 内核结构调试示例

```gdb
p *myproc       # 查看当前进程结构体
p myproc->trapframe
p myproc->pagetable
```

如果你在 `proc.rs` 等处设置断点并想调试当前 `Proc` 的状态，这种方式非常有用。

------

8. 查看内存

```gdb
x/16x 0x80000000     # 查看从物理地址 0x80000000 开始的 16 个字
x/4i $pc             # 查看当前 PC 附近的指令
```
