// error: conditional or transforming attributes can change the frozen ABI
#[zplugin::interface(id = "tests.invalid", version = 1)]
pub trait Bad { #[cfg(unix)] fn run(&self)->zplugin::Result<()>; }
fn main() {}
