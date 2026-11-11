// error: mismatched types
#[allow(non_camel_case_types)]
type u64 = String;

#[zplugin::interface(id = "tests.shadowed-scalar", version = 1)]
trait Bad {
    fn scalar(&self, value: u64) -> Result<u64, zplugin::Error>;
}

fn main() {}
