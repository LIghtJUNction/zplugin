// error: ABI field and variant names must use ASCII identifiers
#[zplugin::abi_type(id="tests.unicode-field",version=1)]
#[derive(Clone, Copy)]
struct Invalid { 数据:u32 }
fn main() {}
