// error: unsupported zplugin parameter type
#[zplugin::interface(id = "tests.invalid", version = 1)]
pub trait Bad { fn run(&self, data: Vec<u8>)->zplugin::Result<()>; }
fn main() {}
