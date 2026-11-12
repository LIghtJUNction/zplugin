// error: ABI values require fixed-width scalars
#[zplugin::interface(id="tests.bad",version=1)]
trait Bad { fn call(&self)->zplugin::Result<zplugin::OwnedTask<&str>>; }
fn main() {}
