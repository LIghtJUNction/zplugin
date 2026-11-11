// error: unsupported zplugin parameter type
#[zplugin::interface(id = "tests.invalid", version = 1)]
pub trait Bad { fn run(&self, text: String)->zplugin::Result<()>; }
fn main() {}
