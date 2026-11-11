// error: zplugin methods require &self or &mut self
#[zplugin::interface(id = "tests.invalid", version = 1)]
pub trait Bad { fn run(value:u32)->zplugin::Result<()>; }
fn main() {}
