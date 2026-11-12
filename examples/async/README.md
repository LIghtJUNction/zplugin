# Rust cross-library async example

Three independent crates demonstrate a real worker-driven task:

- `interface`: allocator-free shared Rust trait returning `OwnedTask<u64>`.
- `plugin`: a native `cdylib` whose future owns a worker, a condition variable and
  its task state; completion and cancellation join the worker.
- `host`: typed loading, Future polling with `park/unpark`, explicit cancellation
  and generation switching. No async-executor dependency is needed.

From the repository root:

```sh
python3 scripts/async.py
```

This builds two plugin generations in separate paths and runs ten contract
groups. The host deliberately triggers one Rust panic to verify containment;
the final JSON reports success only after the process continues through every
check. A test deadline reports a missing notification; it never forces an unload.

To run a single generation manually on Linux:

```sh
cargo build --release -p async-plugin
cargo run --release -p async-host -- target/release/libasync_plugin.so
```

Use `libasync_plugin.dylib` on macOS and `async_plugin.dll` on Windows. The host
accepts an optional second plugin path for hot-generation tests. `--features v2`
builds the second implementation with the same interface and a distinct business
version. The script copies it to an independent path before loading it.

`AsyncService::start(mode, value)` returns a task; `READY` is an immediate result,
`WORKER` waits for `signal()`, and error/panic modes exercise failure handling.
The generated `AsyncServiceProxy::start` returns `Task<u64>`, so an asynchronous
host can use `service.start(READY, 41)?.await?`. The task holds its library even
after the creating proxy is dropped. Borrowed inputs never outlive a factory call.

The test replaces a pending task's waker, waits for a real worker notification,
cancels both unpolled and pending tasks, and races signal with cancel/drop across
32 iterations. It verifies the worker count is zero before cooperative close.
The hot-switch test keeps an old task alive while a new generation serves calls,
then verifies retirement after the old task is destroyed.

```sh
rustup toolchain install 1.85.0 --profile minimal
python3 scripts/async.py --cross-version
```

The cross-version mode builds independent 1.85.0/stable hosts and debug/release
plugins and runs all four combinations. Set `ZPLUGIN_STABLE_TOOLCHAIN=1.99.1` to
reproduce the recorded stable compiler exactly.

The plugin's `UnloadSafe` implementation relies on its explicit worker join.
Reference counts do not prove that a final waker callback has returned from
plugin code. A plugin using Tokio or another executor must own/enter that runtime
and synchronously drain its work, or use pinned loading. zplugin supplies the
task/waker protocol and lifetime wrappers, not an executor or runtime shutdown.
