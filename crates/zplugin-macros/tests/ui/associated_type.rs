// error: associated types and constants are unsupported
#[zplugin::interface(id = "tests.invalid", version = 1)]
pub trait Bad { type Item; }
fn main() {}
