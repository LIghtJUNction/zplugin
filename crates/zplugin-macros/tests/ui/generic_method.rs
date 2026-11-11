// error: synchronous, safe, nongeneric required methods
#[zplugin::interface(id = "tests.invalid", version = 1)]
pub trait Bad { fn run<T>(&self)->zplugin::Result<()>; }
fn main() {}
