// error: required by a bound in `AbiValue`
#[zplugin::abi_type(id="tests.bad",version=1)]
struct Bad { value:u64 }
fn main() {}
