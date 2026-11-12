// error: cannot be sent between threads safely
fn send<T: Send>() {}
fn main() { send::<zplugin::Task<'static, u64>>(); }
