# zplugin-macros

The optional compile-time companion to [zplugin](https://crates.io/crates/zplugin).
Enable `zplugin`'s `macros` feature and use `#[zplugin::interface]`; depend on
`zplugin` directly rather than invoking this implementation crate.

The macro parses a deliberately small Rust interface: borrowed byte/string
inputs, caller-provided byte outputs, fixed-width scalar values, and explicit
`Result` returns. It generates C-compatible argument records, canonical method
descriptions, plugin adapters, and typed host proxies. Unsupported signatures
produce compile-time diagnostics.

`syn`, `quote`, and `proc-macro2` run during compilation only. They are not linked
into the zplugin runtime or the generated dynamic plugin boundary.

