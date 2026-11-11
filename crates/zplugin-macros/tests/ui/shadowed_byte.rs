// error: mismatched types
#[allow(non_camel_case_types)]
type u8 = String;

#[zplugin::interface(id = "tests.shadowed-byte", version = 1)]
trait Bad {
    fn bytes(&self, value: &[u8]) -> Result<(), zplugin::Error>;
}

fn main() {}
