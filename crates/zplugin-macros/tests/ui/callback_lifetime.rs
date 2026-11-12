// error: callback parameters use zplugin::CallbackRef<'_>
#[zplugin::interface(id="tests.bad",version=1)]
trait Bad { fn call(&self, value:zplugin::CallbackRef<'static>)->zplugin::Result<()>; }
fn main() {}
