// error: mismatched types
struct OwnedBytes;

impl OwnedBytes {
    fn into_raw(self) -> zplugin::abi::RawBuffer {
        panic!("this unrelated type must never become an owned ABI result")
    }
}

#[zplugin::interface(id = "tests.shadowed-owned", version = 1)]
trait Bad {
    fn buffer(&self) -> Result<OwnedBytes, zplugin::Error>;
}

fn main() {}
