// error: SecondImplementation: UnloadSafe
#[zplugin::interface(id = "tests.unload.first", version = 1)]
trait First {
    fn run(&self) -> zplugin::Result<()>;
}

#[zplugin::interface(id = "tests.unload.second", version = 1)]
trait Second {
    fn run(&self) -> zplugin::Result<()>;
}

struct FirstImplementation;
impl First for FirstImplementation {
    fn run(&self) -> zplugin::Result<()> {
        Ok(())
    }
}
// SAFETY: this implementation owns no resources or background activity.
unsafe impl zplugin::export::UnloadSafe for FirstImplementation {}

struct SecondImplementation;
impl Second for SecondImplementation {
    fn run(&self) -> zplugin::Result<()> {
        Ok(())
    }
}

fn create_first() -> zplugin::Result<FirstImplementation> {
    Ok(FirstImplementation)
}
fn create_second() -> zplugin::Result<SecondImplementation> {
    Ok(SecondImplementation)
}

zplugin::export_interfaces!(
    version = (1, 0, 0), unload = cooperative;
    FirstExport<FirstImplementation> => create_first,
    SecondExport<SecondImplementation> => create_second,
);

fn main() {}
