// error: cannot be shared between threads safely
fn sync<T: Sync>() {}
fn main() { sync::<zplugin::Instance<'static>>(); }
