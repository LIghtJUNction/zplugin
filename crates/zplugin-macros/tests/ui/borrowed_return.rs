// error: unsupported zplugin return type
#[zplugin::interface(id = "tests.invalid", version = 1)]
pub trait Bad { fn run(&self)->zplugin::Result<&str>; }
fn main() {}
