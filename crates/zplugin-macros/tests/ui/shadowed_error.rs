// error: E0308
struct Error;

#[zplugin::interface(id = "tests.shadowed-error", version = 1)]
trait Bad {
    fn run(&self) -> Result<u64, Error>;
}

fn main() {}
