// error: unsupported ABI value type
#[zplugin::abi_type(id="tests.bad",version=1)]
#[derive(Clone, Copy)]
struct Bad { value:String }
fn main() {}
