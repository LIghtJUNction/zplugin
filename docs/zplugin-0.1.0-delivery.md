# zplugin 0.1.0 交付与发布报告

日期：2026-11-12（Asia/Taipei）

## 发布结果

zplugin 0.1.0 已完成实现、跨平台验收和 crates.io 发布。
GitHub Release 于 2026-11-12 02:01:54（UTC+8）创建，状态为正式发布。

| 项目 | 已核验结果 |
| --- | --- |
| 仓库 | https://github.com/LIghtJUNction/zplugin |
| 发布提交 | `e06f7570eaede6f46b4100ce7c0251e4cf510822` |
| Tag | https://github.com/LIghtJUNction/zplugin/tree/v0.1.0 |
| GitHub Release | https://github.com/LIghtJUNction/zplugin/releases/tag/v0.1.0 |
| 运行时 crate | https://crates.io/crates/zplugin/0.1.0 |
| 过程宏 crate | https://crates.io/crates/zplugin-macros/0.1.0 |
| 发布工作流 | https://github.com/LIghtJUNction/zplugin/actions/runs/43073513725 |
| 初次跨平台验收 | https://github.com/LIghtJUNction/zplugin/actions/runs/43073093462 |
| Google Drive | https://drive.google.com/drive/folders/15G75B0kx4T5JsiDF6XBeBlBzo0Wl4Xbk |

发布工作流和 crates.io sparse index 返回的包 SHA-256 一致：

| 包 | SHA-256 |
| --- | --- |
| zplugin-macros 0.1.0 | `d8c5c63dbdd4a962442524510195b2c706d87f0f67704d5546a145be31ff086d` |
| zplugin 0.1.0 | `17533d9dc7b299454be477150e5bae57be8575a957b6610a171c1679549686fb` |

发布后另建了一个独立消费项目，直接从 crates.io 下载两个 0.1.0 包，
启用 std/macros/hot-reload，生成接口并成功编译运行。这个检查使用 registry
依赖，没有指向工作区源码的 path 依赖。

## 已实现的能力

- 默认 no_std、无需分配器、零第三方 Rust 运行时依赖；alloc、std、macros、
  hot-reload 分别显式启用。系统动态加载器仍由操作系统提供。
- Linux、macOS、Windows 原生动态加载；Windows 提供原生路径和无分配 UTF-16 API。
- `#[zplugin::interface]` 生成描述、C ABI 适配层、精确签名检查和类型明确的代理。
- `export!` 和 `export_interfaces!` 导出一个或多个独立版本接口。
- 支持 8/16/32/64 位有符号与无符号整数、f32/f64、借用字节、可写输出、UTF-8、
  Result/状态码、插件实例及携带正确释放路径的 OwnedBytes。
- 固定 `zplugin_entry_v1`，先检查描述符长度，再读取字段；绑定时检查接口 ID、
  版本、方法数量、函数存在性和完整规范签名。
- 对象由原插件析构；返回 Buffer 独立维持所属库生命周期，可在原实例销毁后使用。
- 保守的单线程模型、同对象重入拒绝、std/unwind panic 捕获及实例 poisoned 状态。
- 分代切换、请求级版本租约、显式 snapshot/restore、失败保持当前版本、旧代驻留上限。
- 协作式 close 和 Busy 回退；没有超时强制卸载。
- 完整共享接口、插件、宿主示例，跨平台 CI，实际性能基准，以及可恢复的版本发布工作流。

实现采用 cdylib、目标平台 C ABI 和显式函数表。插件内部可以使用 Rust 标准库、
泛型、第三方库和异步运行时；跨边界使用已定义的有限表示。原生加载仍保留 unsafe
信任边界，版本检查用于发现不兼容。

## 验收证据

首轮跨平台矩阵和发布流程内的候选提交矩阵全部通过。

| 实测目标 | Rust 1.85.0 | Rust 1.99.1 |
| --- | --- | --- |
| x86_64-unknown-linux-gnu | 通过 | 通过 |
| aarch64-apple-darwin | 通过 | 通过 |
| x86_64-pc-windows-msvc | 通过 | 通过 |

每个完整原生测试任务执行：15 项核心/宏边界/多接口运行时测试、5 项宏解析测试、
21 个编译失败诊断、8 种 feature 配置、8 组真实动态库契约测试及完整示例自测。
其中 10 个畸形动态库由独立 rustc 源文件构建，覆盖缺入口、空入口、错误 ABI、
错误 magic、短描述符、接口版本错误、短接口描述、签名错误、缺失方法及 Busy。

fmt、严格 clippy、2 个可执行 doctest、13 项发布事务测试均通过。
3 个需要独立共享 crate 上下文的 README 片段标记为 ignored，对应功能由完整示例
工作区执行覆盖。两个发布包均完成真实打包、独立编译和发布 dry-run。

Rust 1.85.0 与 1.99.1 独立编译的宿主/插件四种组合均通过，包含 debug 与 release
插件。thumbv7em-none-eabi 和 wasm32-unknown-unknown 的 core-only 编译通过；
该检查验证协议可移植性，不宣称这两个目标拥有原生动态加载器。

## 实测性能

环境：AMD EPYC 9V74，Linux x86-64，共享执行环境，固定 CPU affinity；
Rust 1.99.1，release、thin LTO、codegen-units=1、panic=unwind。
三个独立进程轮换基线顺序；下表为三轮中位数的中位数。

| 标量调用路径 | ns/次 |
| --- | ---: |
| 静态可内联 | 1.354 |
| 静态不内联 | 1.635 |
| 缓存的薄 extern C 函数指针 | 2.508 |
| 缓存的完整生成适配器 | 3.789 |
| libloading 缓存相同适配器 | 3.665 |
| zplugin 完整类型代理 | **3.644** |

完整代理与缓存 libloading 对照使用相同适配器、对象、线程和库生命周期语义。
静态及薄 C 对照用于给出更低的工作量基线。3.644 ns 与 3.665 ns 的差值处于
共享机器波动范围内；这组结果支持低额外调用开销。

| 完整代理/生命周期操作 | 实测中位数 |
| --- | ---: |
| 64 B ASCII 转换至复用输出 | 13.523 ns |
| 4 KiB ASCII 转换至复用输出 | 169.580 ns |
| 1 MiB ASCII 转换至复用输出 | 37.771 μs |
| 接口绑定 | 29.832 ns |
| 对象创建＋销毁 | 17.759 ns |
| 64 B 拥有型输出创建＋释放 | 43.189 ns |
| 已映射库的 core 加载＋释放引用 | 163.340 ns |
| 重新映射加载＋关闭 | 35.188 μs |
| 完整加载、绑定、创建、切换与回收 | 29.318 μs |

全部标量和借用字节调用的三轮记录中，宿主与插件 Rust 堆分配次数均为 **0**。
示例对象申请 16 字节插件堆内存，借用方式创建时宿主堆申请为 0；连续创建 1,024
个对象申请 16,384 字节，销毁后回到原有 live-byte 数量。
该配置下 Plugin 为 32 字节、Instance 为 72 字节、含 8 个缓存方法的代理为 136 字节。
这些大小和耗时属于本次目标/配置；操作系统加载器内部分配不在 Rust 分配计数内。

完整采样、批次及单次延迟分布、计时器底噪、运行波动、RSS 增量和复现步骤：
https://github.com/LIghtJUNction/zplugin/blob/v0.1.0/docs/performance.md

## 支持边界

对象、代理和返回缓冲采用 !Send、!Sync；同对象调用由可变代理序列化。
首版不提供跨边界 async task、waker 或回调协议，也不传递原生 String、Vec、
Box<dyn Trait>、Arc、闭包或 Future 布局。插件内部使用这些能力不受接口宏限制。

默认 pinned 保留映像。物理 close 需要协作式契约覆盖线程、任务、回调、TLS/退出
析构和所有返回资源；Busy 时继续保留。OS 或其他加载器持有的引用可以使映像在
成功释放本库引用后继续驻留。panic=abort 和本机崩溃保持进程级行为。
详细契约：https://github.com/LIghtJUNction/zplugin/blob/v0.1.0/docs/abi.md

## 使用与复现

```sh
cargo add zplugin --features std,macros,hot-reload
git clone https://github.com/LIghtJUNction/zplugin.git
cd zplugin
git checkout v0.1.0
cargo build --release -p demo-plugin
cargo run --release -p demo-host -- target/release/libdemo_plugin.so --self-test
python3 scripts/check.py
python3 scripts/check.py --cross-version
python3 tools/bench/run.py
```

macOS 使用 libdemo_plugin.dylib，Windows 使用 demo_plugin.dll。
MSRV 和 stable 都需要预先安装，交叉编译复现步骤见验证文档。

后续发布通过 Release workflow_dispatch 选择 patch/minor/major/custom；
custom 严格校验 SemVer，dry_run 不发布。流程同步工作区版本和精确内部依赖、
Cargo.lock、变更记录，在同一候选提交通过矩阵后原子推送 tag，再按依赖顺序发布。
CARGO_REGISTRY_TOKEN 仅注入上传步骤。部分发布可使用同一不可变 tag 的 custom
版本重试，已存在且校验和不同的包会被拒绝。

## 提交记录与复核

- `010065e`：初始化 Project Z 开发契约。
- `aaa3464`：no_std ABI、加载器、生成接口和首批契约测试。
- `47ef982`：完整分代回收、多接口、真实动态库测试、基准、CI 和发布事务。
- `e06f757`：记录跨平台验收并发起 0.1.0 正式发布；v0.1.0 指向该提交。

五项复核：能力对应公开 API；术语对应实际记录和生命周期；性能结论对应原始 CSV；
验收包含独立编译、畸形输入和失败释放路径；信任、线程及物理卸载边界明确。
本次授权范围内的开发、验收、发布和报告同步均已完成，无外部阻塞。
