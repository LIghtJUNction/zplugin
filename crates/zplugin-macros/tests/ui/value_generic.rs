// error: ABI value declarations must be nongeneric
#[zplugin::abi_type(id="tests.bad",version=1)]
#[derive(Clone, Copy)]
struct Bad<T:Copy> { value:T }
fn main() {}
