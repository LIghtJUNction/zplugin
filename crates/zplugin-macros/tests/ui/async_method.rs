// error: synchronous, safe, nongeneric required methods
#[zplugin::interface(id = "tests.invalid", version = 1)]
pub trait Bad { async fn run(&self)->zplugin::Result<()>; }
fn main() {}
