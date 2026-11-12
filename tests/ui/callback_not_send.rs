// error: cannot be sent between threads safely
fn require_send<T: Send>() {}
fn main() {
    require_send::<zplugin::Callback>();
    require_send::<zplugin::CallbackRef<'static>>();
}
