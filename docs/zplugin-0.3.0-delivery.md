# zplugin 0.3.0：不可信 Wasm 插件交付报告

日期：2026-11-13，Asia/Taipei。

## 结论与范围

**zplugin 0.3.0 和 zplugin-macros 0.3.0 已发布到 crates.io，并通过公开包的独立安装验收。**
本轮增加可选的 Wasm 沙箱加载后端，用于运行不可信插件；原生动态库加载仍只适用于可信代码。
发布提交为 [`d10e9082495c8dafa2b0b0f3d19810cf5926ef02`](https://github.com/LIghtJUNction/zplugin/commit/d10e9082495c8dafa2b0b0f3d19810cf5926ef02)，
同一提交经过验证、打包、创建 tag 和发布，没有跳过发布门禁。

| 路径 | 适用插件 | 本轮交付与边界 |
| --- | --- | --- |
| 原生动态库 | 可信 Rust、C、C++、Zig 插件 | 保留既有同步、异步任务、稳定值、资源和回调；不提供恶意代码隔离 |
| 可选 Wasm | 遵循 zplugin Wasm v1 协议的核心 Wasm32 二进制 | 校验模块、拒绝全部导入和 start section、限制执行预算和客体资源；安全的宿主加载 API |
| 任意 Wasm 应用 | WASI、依赖 JavaScript 的 wasm-bindgen、Component Model 等 | 本版不支持；文件扩展名为 `.wasm` 不等于协议兼容 |

Wasm 边界只传操作码和字节缓冲，不向客体暴露宿主地址、分配器、函数指针或 Rust 布局。
没有链接文件、网络、环境变量、时钟、进程或 WASI 能力。各实例拥有独立状态，宿主类型
为 `!Send + !Sync`，通过可变借用串行调用。插件可以保留自己收到的数据；不同租户或
机密会话应使用新实例，不能把实例复用理解为数据遗忘。

## 使用

宿主显式启用沙箱：

```toml
[dependencies]
zplugin = { version = "0.3.0", features = ["wasm"] }
```

```rust,ignore
use zplugin::wasm::{Interface, Limits, Plugin};

let interface = Interface { id: 0x7a70_6c75_6769_6e01, version: 1 };
let mut plugin = Plugin::new(&bytes, interface, Limits::default())?;
let mut output = [0; 64];
let written = plugin.call(1, b"hello", &mut output)?;
assert_eq!(&output[..written], b"HELLO");
# Ok::<(), zplugin::wasm::Error>(())
```

这里的 `bytes` 是已按模块大小上限读取的二进制。操作 `1` 是示例插件的转大写方法，
不是库内置的业务功能。完整[客体实现](https://github.com/LIghtJUNction/zplugin/blob/v0.3.0/examples/wasm/guest.rs)
使用 `wasm_guest::Guest` 和 `export_wasm!`，无需分配器；
[宿主示例](https://github.com/LIghtJUNction/zplugin/blob/v0.3.0/examples/wasm_host.rs)包含有界文件读取与失败用例。

```sh
rustup target add wasm32-unknown-unknown
python3 scripts/wasm.py
```

默认/core 与原生运行时仍是 `no_std`、零第三方运行时依赖，MSRV 为 Rust **1.85**。
可选 `wasm` 宿主需要 Rust **1.86**，隐式启用 `alloc`、不隐式启用 `std`，增加 Wasmi
2.0.0 及其依赖共 **8 个第三方运行时包**。开发与稳定版 CI 使用 Rust **1.100.0**、edition **2024**。
编译期宏依赖与运行时依赖继续分开，不将“默认零依赖”宣传成“所有功能零依赖”。

## 限制和错误处理

| 资源 | 默认上限 |
| --- | ---: |
| 模块二进制 | 4 MiB，解析前检查 |
| 客体线性内存 | 16 MiB，最多一个 memory |
| 表元素 | 4,096，最多一个 table |
| 输入、输出协商容量 | 各 1 MiB |
| 执行预算 | 10,000,000 fuel；所有元数据查询共享一次预算，每次业务调用重新赋予预算 |
| 解释器值栈、调用深度 | 64 KiB、128 层 |

内存偏移、长度、缓冲区重叠、接口 ID/版本及输出长度都经过检查。调用前清零允许写入的
客体输出区；仅在成功且长度合法后复制回宿主。任何错误都保持调用者输出不变。
业务错误不自动作废实例；trap、预算耗尽或非法成功长度会将实例永久标记为 `Poisoned`。
释放实例不执行客体析构代码。超限 `memory.grow`/`table.grow` 返回 Wasm 的正常失败值，
允许客体自行处理。

这是依赖 Wasmi 正确性的**进程内沙箱**，不是操作系统进程隔离。客体内存上限不是整个宿主
RSS 上限；fuel 不限制解析/编译的墙钟时间，也不消除引擎漏洞、宿主 OOM 或硬件侧信道。
高风险多租户服务仍应叠加独立进程/容器与系统资源策略，并限制并发实例和编译请求数量。
没有声称此适配层或 Wasmi 2.0 经过独立安全审计。

Wasm 当前是同步字节缓冲协议。既有原生异步任务、waker、资源句柄、回调和 trait 代理
**不会自动跨入沙箱**；也未提供 WASI 授权或宿主调用桥。任何语言只要生成相同协议可兼容，
但本轮实际交付的 Wasm SDK 和独立编译示例是 Rust。现有 C/C++/Zig 生成器仍面向原生 ABI。

## 测试证据

[正式发布工作流 43382342689](https://github.com/LIghtJUNction/zplugin/actions/runs/43382342689)
的计划、8 个验证作业和发布作业，合计 **10 个作业全部成功**。

| 验证 | 实际结果 |
| --- | --- |
| Wasm 契约与对抗输入 | 30 项通过：畸形模块、类型错误、导入/start 拒绝、ABI/偏移/长度、fuel、栈、memory/table、trap、状态隔离及变异输入 |
| Cargo feature 合并 | 显式启用引擎 `memory64` 后，适配层仍拒绝 memory64；回归测试通过 |
| 真实 Rust Wasm 插件 | 每个平台执行 8 组自检，含空、64 B、4 KiB、1 MiB、状态、错误、无限循环和 trap |
| 独立 Rust 编译 | 客体/宿主 1.86 与 1.100 的四种组合全部通过 |
| 原生回归 | 三平台原生同步/异步插件、C11/C++17/Zig、旧版 ABI 兼容及打包门禁通过 |
| 编译器与特性 | 原生 MSRV 1.85 的 8 种组合，稳定版的 12 种组合；36 个编译失败诊断案例通过 |
| 常规质量门禁 | fmt、Clippy、单元/集成/文档测试、严格 rustdoc、27 项 Python 工具测试通过 |
| no_std | core 在 thumb/wasm32 编译；可选 Wasm 宿主的 `no_std + alloc` 在 thumb、Rust 1.86 编译通过 |

不同版本重复执行同一测试不会相加宣传为额外独立案例。编译通过的嵌入式目标不代表已在
真机执行沙箱，更不代表这些目标能加载 `.so`/`.dll`。

| 实际宿主 target | 系统 | 原生 Rust 1.85 / 1.100 | Rust 1.100 执行 Wasm |
| --- | --- | --- | --- |
| `x86_64-unknown-linux-gnu` | Linux，glibc 2.43 | 通过 / 通过 | 8 组通过 |
| `aarch64-apple-darwin` | macOS 26.7.2 arm64 | 通过 / 通过 | 8 组通过 |
| `x86_64-pc-windows-msvc` | Windows Server 2025 | 通过 / 通过 | 8 组通过 |

发布后又建立两个独立 Cargo 工作区，锁定公开 registry 的 `zplugin = "=0.3.0"`，没有
path 替代。客体只依赖 zplugin；宿主解析到上述 8 个引擎包。两端 fmt、Clippy、release
构建以及真实调用的 **8 组自检全部通过**，耗时 35.477 秒（含构建，不是调用延迟）。
锁文件中的 zplugin 校验值与发布包一致。独立验收包使用普通 Cargo 客体栈配置，其模块
为 1,016 B；它不是性能报告中设置 64 KiB 客体栈的 993 B 模块，两者不能混用测量数据。

完整 CI 元数据、下载工件的校验值、发布清单和独立安装记录见
[机器可读证据](evidence/0.3.0.json)。

## 实测性能

Linux x86-64、AMD EPYC 9V74，Rust 1.100，优化级别 3、thin LTO、单 codegen unit，
固定 CPU 0。三次独立进程，每项 31 个批次；下表为三个进程的批次平均耗时 p50 的中位数，
**不是单次调用 P99 或实时保证**。对照使用相同 Wasmi 引擎、预算、缓冲清零、复制和生命周期。

| 操作 | zplugin | 缓存 Wasmi 函数对照 |
| --- | ---: | ---: |
| 加载、校验、绑定、释放 | 44.196 µs | 43.098 µs |
| 空调用 | 145.229 ns | 139.701 ns |
| 64 B 往返 | 176.627 ns | 164.530 ns |
| 4 KiB 往返 | 321.324 ns | 299.942 ns |
| 1 MiB 往返 | 78.186 µs | 81.052 µs |

表中预热后的复用缓冲调用均记录到 **0 次 Rust 堆分配**；加载/释放每轮为 121 次分配。
示例预留两个 1 MiB 缓冲，实际客体线性内存 **2,228,224 B**，绑定后保留的请求分配字节
**2,247,430 B**。这些不是进程 RSS，也不是任意插件的最低内存或零分配承诺。
边界有复制；较大缓冲结果受机器噪声影响，不宣称比底层引擎普遍更快。

[完整性能报告](performance-0.3.0.md)提供不可变测量提交、样本量、分配计数范围、
全部原始 CSV、误差范围及复现命令。原生调用性能继续引用独立的 0.2.0 报告，不混用
原生无隔离调用和 Wasm 沙箱的安全语义。

## 发布与归档

发布时间：**2026-11-13 15:05:38（Asia/Taipei）**。

- [功能提交 554878b](https://github.com/LIghtJUNction/zplugin/commit/554878b61a2a67d2f879294db0aa55f069cfe4ee)，[示例栈优化 e957b45](https://github.com/LIghtJUNction/zplugin/commit/e957b45e6f9a071a8ce3030fd75d3ca34334c71b)，[实测文档 63e0334](https://github.com/LIghtJUNction/zplugin/commit/63e0334c0814b87714777e23a44edcab8f28c6de)。
- [发布提交 d10e908](https://github.com/LIghtJUNction/zplugin/commit/d10e9082495c8dafa2b0b0f3d19810cf5926ef02)，[tag v0.3.0](https://github.com/LIghtJUNction/zplugin/tree/v0.3.0)，[GitHub Release](https://github.com/LIghtJUNction/zplugin/releases/tag/v0.3.0)。
- crates.io：[zplugin 0.3.0](https://crates.io/crates/zplugin/0.3.0)、[zplugin-macros 0.3.0](https://crates.io/crates/zplugin-macros/0.3.0)。
- [Google Drive 交付目录](https://drive.google.com/drive/folders/1LBJZoXa6T7_x_JcR3bOaA_KgWxAQfzx2)：协议、性能报告、全部原始 CSV、最终证据和可复现的 registry 安装验收包。

| 公开包 | SHA-256 |
| --- | --- |
| zplugin 0.3.0 | `ea2485a9c61ce94d77f52baf55dcad088a151a7c742125f7a7c1cfee6bd07b4e` |
| zplugin-macros 0.3.0 | `87b3be200d7786220b9993f3c78fb40d2a255e4fb46b6c8451ddd01fc285725b` |

发布工作流保留 patch/minor/major/custom、dry-run、并发互斥、已发布版本检查及部分
失败恢复路径。Token 只注入实际发布步骤；此次未读取或持久化凭据。发布后报告是独立
文档提交，不修改已发布 tag 或包内容。

文档按照 Research-Paper-Writing-Skills 的结论—方法—证据原则组织，将实现能力、实测
结果和支持边界分开；未把未实施的异步 Wasm、WASI 授权或进程级隔离写成现有能力。
