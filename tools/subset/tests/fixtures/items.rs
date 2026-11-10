struct Pair(u32, u32);
struct Marker;
pub struct Open {
    pub(crate) field: u32,
}
enum Code {
    A = 1,
}
trait Named {
    fn name(&self) -> String;
}
impl Named for Marker {
    fn name(&self) -> String {
        String::new()
    }
}
impl Marker {
    fn bump(&mut self) {}
}
static NAME: &str = "x";
type Alias = Marker;
macro_rules! nothing {
    () => {};
}
use std::io::*;
use std::fmt::Result as FmtResult;
const fn zero() -> u32 {
    0
}
async fn later() {}
unsafe fn raw() {}
