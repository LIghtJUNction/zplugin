// error: cannot return value referencing local variable
fn escape(raw:zplugin::abi::RawTask) -> zplugin::Task<'static, u64> {
    let plugin = unsafe { zplugin::Plugin::load(c"plugin") }.unwrap();
    let instance = plugin.bind(zplugin::InterfaceSpec { id:"a", version:1, signatures:&[] }).unwrap().create().unwrap();
    unsafe { instance.task(raw) }.unwrap()
}
fn main() {}
