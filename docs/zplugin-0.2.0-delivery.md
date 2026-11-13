# zplugin 0.2.0 交付报告

日期：2026-11-13，Asia/Taipei。

## 本轮范围

**zplugin 0.2.0 与 zplugin-macros 0.2.0 已正式发布到 crates.io**，并完成公开包的独立安装验收。
本轮新增跨库异步任务、C/C++/Zig 接口生成，以及稳定结构体、枚举、拥有型资源和回调。
最终提交通过全部三平台门禁，tag、Release 和两个公开包使用同一已验证版本。
核心仍是默认 `no_std`、无需分配器、零第三方运行时依赖的原子化库，不包含执行器、
插件市场、目录监听器或业务框架。

| 能力 | 已交付实现 | 明确边界 |
| --- | --- | --- |
| 跨库异步 | `OwnedTask<T>`、`Task<T>`、poll/cancel/release、可 retain 的线程安全 waker，宿主可 `.await` | `T: AbiValue`；不传递原生 Future/Waker 布局，不安装异步运行时 |
| 跨语言生成 | 共享 Rust trait 生成完整 C 头文件、布局、方法签名与描述符初始化器；Zig 翻译该头文件 | 同一目标平台 C ABI；不是跨平台通用二进制 |
| 稳定值 | `#[abi_type]` 结构体、嵌套结构体、无载荷及带载荷枚举，完整递归 schema 检查 | 首版不接受泛型、引用、数组、拥有型容器、bool/char/usize/isize/128 位字段 |
| 拥有型资源 | `OwnedResource`、宿主 `Resource`、资源 ID/版本、原库释放函数和独立存活凭据 | 应用定义操作码及缓冲协议；不跨边界传 Rust 分配器或 trait object |
| 反向回调 | `Callback`/`CallbackRef`、显式 retain/release、重入检查和 panic 污染状态 | 普通回调限创建线程；只有 waker 的通知操作允许跨线程 |

旧入口 `zplugin_entry_v1`、既有同步描述符和方法签名未改变。扩展表使用独立的长度与
版本前缀；应用新增方法仍应升级业务接口版本，不能偷偷改变已冻结的函数表。

## 使用与复现

安装时按需要显式启用功能：

```sh
cargo add zplugin@0.2.0 --features std,macros,hot-reload
```

只需要协议或手工句柄时，可以关闭所有默认功能；`alloc` 提供 Rust future、闭包回调、
拥有型资源等便利适配，`std` 额外支持可展开 panic 的捕获。过程宏的 `syn`、`quote`、
`proc-macro2` 及其传递依赖只用于编译，不是运行时依赖。

完整独立项目与构建步骤：

- [Rust 异步共享接口、插件和宿主](https://github.com/LIghtJUNction/zplugin/tree/v0.2.0/examples/async)。
- [C/C++/Zig 插件和 Rust 宿主](https://github.com/LIghtJUNction/zplugin/tree/v0.2.0/examples/foreign)。
- [扩展 ABI、所有权、线程和取消协议](https://github.com/LIghtJUNction/zplugin/blob/v0.2.0/docs/extensions.md)。

```sh
python3 scripts/check.py
python3 scripts/check.py --cross-version
python3 scripts/legacy.py
python3 scripts/foreign.py --install-zig
python3 tools/bench/run.py docs/benchmarks/v0.2.0
python3 tools/bench/extensions.py
```

## 测试证据

本地使用 Rust **1.100.0** 和 MSRV **1.85.0**；edition 为 **2024**。1.100.0 已核对
[Rust 官方发布记录](https://blog.rust-lang.org/2026/11/12/Rust-1.100.0/)。
两版本均通过完整本地检查，新增基准也通过两个版本的 Clippy；最终 Python 套件为 23 项。

| 验证项 | 结果及覆盖 |
| --- | --- |
| 运行时/契约测试 | 45 项通过，覆盖所有权、错误输出、描述符长度、重入、panic、waker 失败和资源清理 |
| 宏单元测试 | 7 项通过 |
| 文档测试 | 3 项通过；3 个依赖独立共享 crate 的片段由完整示例工程覆盖 |
| core-only 测试 | 9 项通过；不与全功能测试重复相加宣传总量 |
| 编译失败诊断 | 36 个案例通过，包括不支持的签名、生命周期与 Send/Sync 限制 |
| feature 组合 | 8 种显式组合通过 |
| 同步动态库 | 8 组真实动态库契约，另有示例自检 |
| 异步动态库 | 10 组契约，包含工作线程唤醒、取消、析构、旧新代并存和 32 轮唤醒/取消竞态 |
| 独立 Rust 编译 | 同步和异步宿主/插件分别使用 1.85.0 与 1.100.0，覆盖四种组合与 debug/release |
| 旧版兼容 | 对固定 v0.1.0 源码进行旧宿主/新插件、新宿主/旧插件双向真实加载 |
| Python 验证/发布工具 | 23 项通过，覆盖版本计算、候选一致性、发布恢复、UTF-8、超时及平台链接命令 |
| 打包 | 本地两 crate 联合打包、解包编译及 publish dry-run 通过；正式候选由发布门禁重新验证 |

嵌入式 `thumbv7em-none-eabi` 和 `wasm32-unknown-unknown` 检查的是无分配器协议的
编译能力，不代表这些目标可以加载本机 `.so`、`.dylib` 或 `.dll`。

最终候选的完整三平台门禁已全部通过：
[Release 工作流 43376384241](https://github.com/LIghtJUNction/zplugin/actions/runs/43376384241)
共 7 个验证作业，连同计划和发布作业共 9 项成功。

| 实际执行 target | Rust 1.85.0 | Rust 1.100.0 | C11/C++17/Zig 原生库 |
| --- | --- | --- | --- |
| `x86_64-unknown-linux-gnu` | 通过 | 通过 | 三个 `.so` 通过，Zig 0.17.1 |
| `aarch64-apple-darwin` | 通过 | 通过 | 三个 `.dylib` 通过，Zig 0.15.2 |
| `x86_64-pc-windows-msvc` | 通过 | 通过 | 三个 `.dll` 通过，Zig 0.17.1 |

原生 CI 机器分别是 Linux/glibc 2.43、macOS 26.7.2 arm64、Windows Server 2025。
C/C++ 编译器分别为 GCC 15.2.0、Apple Clang 21.0.0、Zig Clang 22.1.8。
跨语言测试分别耗时 70.36、41.50、202.03 秒，包含工具准备、编译与实际加载，不是调用性能数字。
原始机器记录和校验值保存在 [机器可读验收证据](evidence/0.2.0.json)。

## 实测性能

所有数字来自 Linux x86-64、AMD EPYC 9V74、Rust 1.100.0、release/thin LTO 的三次独立进程。
下表是三次运行中位数的中位数；普通 p50/p95/p99 是**批次平均耗时的分位数**，不是单次请求
的尾延迟。同步基准与异步基准没有并行运行，也没有与本任务编译任务重叠。

| 操作 | 实测中位数 | Rust 堆分配 |
| --- | ---: | ---: |
| 同步完整代理，标量 | 3.474 ns/次 | 0 |
| 同步代理，64 B | 13.370 ns/次 | 0 |
| 同步代理，4 KiB | 174.716 ns/次 | 0 |
| 同步代理，1 MiB | 38.279 µs/次 | 0 |
| 异步缓存 C poll | 12.216 ns/次 | 0 |
| 异步完整代理 poll，复用 C waker | 14.018 ns/次 | 0 |
| Rust Future poll，复用执行器 waker | 14.740 ns/次 | 0 |
| Ready 任务创建/销毁 | 20.741 ns/次 | 1 次，56 B |
| 每轮更换 C waker | 45.153 ns/次 | 1 次，40 B |
| 每轮更换 Rust waker | 73.983 ns/次 | 2 次，80 B |
| 加载、绑定、创建、切代、回收 | 26.386 µs/次 | 3 次宿主分配；不含独立插件计数 |

异步代理在相同 C waker 和生命周期语义下比缓存 C poll 多 **1.802 ns/次**。工作线程
创建/取消/join/释放的三次 p50 为 8.478–14.242 µs；这些结果受调度影响，不构成取消时限。
异步测量每行净存活字节增量为零，结束时插件计数字节归零，并验证协作式关闭成功。

创建、首次桥接和更换 waker 会分配，不能把“稳定 poll 为零分配”扩展成“所有异步操作零分配”。
计数器不覆盖系统加载器或 OS 线程内部的分配。4 KiB/1 MiB 的运行间波动明显，不据此宣称
比所有缓存函数指针或 libloading 普遍更快。

[完整性能报告](https://github.com/LIghtJUNction/zplugin/blob/v0.2.0/docs/performance-0.2.0.md)
包含全部对照、分配/内存、加载、绑定、创建/销毁、热切换、延迟测量方法和六份原始 CSV。
测量在版本号递增前完成；最终发布仅做版本同步，运行时及示例的功能源码没有另行替换。

## 安全与支持边界

加载本机代码仍是明确的 `unsafe`/信任边界。ABI 检查发现不兼容，不隔离恶意代码、
内存破坏、`panic=abort` 或本机崩溃。插件自己负责网络、文件、运行时、线程及业务状态。

普通代理、任务的 poll/cancel/drop、资源和一般回调均采用创建线程模型，不声明 Send/Sync。
waker retain/wake/release 可以跨线程。异步任务必须拥有调用结束后仍需要的数据，不能把
调用期借用保存到 future。异步结果目前限 `AbiValue`，不直接返回拥有型缓冲或资源。

默认固定驻留最保守。物理卸载依赖显式的协作式安全承诺：必须释放对象、资源、任务和回调，
并等待所有工作线程、注册项以及最后一次回调/析构的返回路径退出。示例通过 join 实现；
不能因为引用计数为零就假定任意外部执行器已经停机。清理失败时保留库，不用超时强制卸载。

C/C++/Zig 示例使用平台系统 ABI。Zig 验证固定 Linux/Windows 0.17.1、macOS 0.15.2；
macOS 由 Zig 生成 PIC 对象，再由当前 Apple SDK 的 C 驱动链接。测试某一系统版本不代表
已验证所有更旧系统，交付插件必须遵守自己的目标架构、部署版本和系统库要求。

## 发布与归档

发布时间为 **2026-11-13 14:13:05（Asia/Taipei）**。

| 项目 | 结果 |
| --- | --- |
| 已验证版本提交 | [`9870ed1873019063b61fdfb3bb2bd9a5ecb858ad`](https://github.com/LIghtJUNction/zplugin/commit/9870ed1873019063b61fdfb3bb2bd9a5ecb858ad) |
| Tag | [`v0.2.0`](https://github.com/LIghtJUNction/zplugin/tree/v0.2.0) |
| GitHub Release | [zplugin 0.2.0](https://github.com/LIghtJUNction/zplugin/releases/tag/v0.2.0) |
| 运行时 crate | [zplugin 0.2.0](https://crates.io/crates/zplugin/0.2.0) |
| 编译期宏 crate | [zplugin-macros 0.2.0](https://crates.io/crates/zplugin-macros/0.2.0) |

registry 返回的版本均未撤回，下载包与发布记录的 SHA-256 一致：

| 包 | SHA-256 |
| --- | --- |
| zplugin | `2376c2084d78c80311487ea9658db1e5d999a05374264318f9869ae56725d14e` |
| zplugin-macros | `8b38bdcb1dc20733d9f4ea369fa45cd8576d3bf7bfc03bb48d34068880904a99` |

发布由同一工作流完成版本校验、工作区和内部依赖同步、最终候选验证、原子提交/tag 推送、
依赖顺序上传、registry 可见性等待与校验、GitHub Release 创建。`CARGO_REGISTRY_TOKEN`
只注入上传步骤；保留 patch/minor/major/custom、dry_run、互斥及部分发布恢复。

本轮已分阶段推送，主要实现记录为：

- [`abf2511`](https://github.com/LIghtJUNction/zplugin/commit/abf25119fe67361648b4863aea1860d94be604d6)：任务、稳定值、资源和回调协议。
- [`290ab2e`](https://github.com/LIghtJUNction/zplugin/commit/290ab2e79872d1208a06f91d20a0b04a9aaee8d3)：Rust 异步及 C/C++/Zig 示例、独立版本兼容和跨平台测试。
- [`577e631`](https://github.com/LIghtJUNction/zplugin/commit/577e6315b4f28f9e8fa43525dc28a44476bb4820)：分配计数任务基准、三次实测原始数据和性能报告。
- [`1601d41`](https://github.com/LIghtJUNction/zplugin/commit/1601d41b56910a19ba7b1cba7e7a6db1db0df8e8)：macOS 原生链接链路及对应回归测试。

发布后的 registry-only 独立共享接口/插件/宿主工程也已通过。它从 crates.io 下载两个
0.2.0 包，确认 Cargo source 为 registry、两个校验值匹配，并通过 release 构建、fmt、
Clippy 和 8 组动态调用。覆盖嵌套结构体、载荷枚举、异步 Pending/wake/Ready、取消、
保留回调、资源超出代理生命周期存活、恰好一次释放和最后一个库存活凭据退出。
没有使用本地 zplugin 路径覆盖来代替公开包。

云盘归档：[zplugin-0.2.0-delivery-2026-11-13](https://drive.google.com/drive/folders/1jrFug_0CriFpyoAExv8KTrU69yhxBb60)。
归档包含本报告、性能报告、ABI/扩展协议、六份原始 CSV、机器可读 CI 证据，以及独立
公开包消费工程和验收记录。GitHub 保存源码和报告，云盘提供交付副本。

## 结论与证据自检

- 功能贡献：提供稳定边界与生命周期原语，不包装成完整异步框架。
- 表达清晰：接口、ABI、crate 和业务版本分开；借用、拥有、通知和执行分开。
- 实验强度：保留静态/缓存 C/libloading 对照，初始化与稳定调用分测，公开不利波动。
- 验证范围：真实原生库、跨 Rust 版本、错误与资源退出路径均有可执行测试；最终原生矩阵和公开包安装均已通过。
- 设计正确性：不承诺自动关闭任意执行器、强制卸载、不可信代码隔离或未测平台性能。

文档依据 Research-Paper-Writing-Skills 的结论—证据对应原则整理：实现、实测和支持边界
分别陈述；未经执行的能力不记为通过。
