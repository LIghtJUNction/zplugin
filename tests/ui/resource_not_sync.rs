// error: cannot be shared between threads safely
fn require_sync<T: Sync>() {}
fn main() {
    require_sync::<zplugin::OwnedResource>();
    require_sync::<zplugin::Resource<'static>>();
}
