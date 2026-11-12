// error: ABI field name is reserved in C/C++
#[zplugin::abi_type(id="tests.reserved",version=1)]
#[derive(Clone, Copy)]
struct Invalid { and:u32 }
fn main() {}
