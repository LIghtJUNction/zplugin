// error: synchronous, safe, nongeneric required methods
#[zplugin::interface(id = "tests.invalid", version = 1)]
pub trait Bad { fn run(&self)->zplugin::Result<()> { Ok(()) } }
fn main() {}
