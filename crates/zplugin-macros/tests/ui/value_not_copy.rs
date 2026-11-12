// error: the trait bound `Bad: Copy` is not satisfied
#[zplugin::abi_type(id="tests.bad",version=1)]
struct Bad { value:u64 }
fn main() {}
