// error: unsupported zplugin parameter type
#[zplugin::interface(id = "tests.invalid", version = 1)]
pub trait Bad { fn run(&self, n: usize)->zplugin::Result<()>; }
fn main() {}
