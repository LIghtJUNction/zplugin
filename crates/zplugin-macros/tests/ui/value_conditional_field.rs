// error: conditional or transforming attributes can change the frozen ABI
#[zplugin::abi_type(id="tests.bad",version=1)]
#[derive(Clone, Copy)]
struct Bad { #[cfg(unix)] value:u64 }
fn main() {}
