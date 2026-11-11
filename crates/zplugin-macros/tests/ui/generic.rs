// error: safe nongeneric traits
#[zplugin::interface(id = "tests.invalid", version = 1)]
pub trait Bad<T> { fn run(&self)->zplugin::Result<()>; }
fn main() {}
