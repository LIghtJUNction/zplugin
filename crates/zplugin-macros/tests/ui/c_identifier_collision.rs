// error: method names collide after C identifier encoding
#[zplugin::interface(id="tests.name-collision",version=1)]
trait Invalid {
    fn é(&self)->zplugin::Result<u64>;
    fn _ue9_(&self)->zplugin::Result<u64>;
}
fn main() {}
