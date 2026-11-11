// error: zplugin errors must use zplugin::Error
#[zplugin::interface(id = "tests.invalid", version = 1)]
pub trait Bad { fn run(&self)->Result<u32, ()>; }
fn main() {}
