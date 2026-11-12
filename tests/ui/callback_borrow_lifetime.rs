// error: cannot return value referencing temporary value
fn escape() -> zplugin::CallbackRef<'static> {
    zplugin::Callback::new(|_, _| Ok(0)).as_ref()
}
fn main() {}
