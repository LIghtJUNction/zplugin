// error: cannot return value referencing local variable
fn escape() -> zplugin::Instance<'static> {
    let plugin = unsafe { zplugin::Plugin::load(c"plugin") }.unwrap();
    plugin.bind(zplugin::InterfaceSpec { id: "a", version: 1, signatures: &[] }).unwrap().create().unwrap()
}
fn main() {}
