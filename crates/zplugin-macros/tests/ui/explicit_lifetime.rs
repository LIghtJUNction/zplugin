// error: explicit boundary lifetimes are unsupported
#[zplugin::interface(id = "tests.invalid", version = 1)]
pub trait Bad { fn run(&self, data: &'static [u8])->zplugin::Result<()>; }
fn main() {}
