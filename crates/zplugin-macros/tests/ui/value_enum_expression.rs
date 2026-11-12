// error: ABI enum tags must be u32 integer literals
#[zplugin::abi_type(id="tests.bad",version=1)]
#[derive(Clone, Copy)]
enum Bad { One = 1 + 1 }
fn main() {}
