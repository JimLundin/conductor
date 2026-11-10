fn types<'a, T>(flag: bool, it: impl Iterator, out: &mut u32, s: &'a str) -> impl Iterator
where
    T: Clone,
{
    it
}

struct Holder {
    any: Box<dyn Fn()>,
    pair: (u32, u32),
    grid: [u8; 4],
    slice: &'static [u8],
    nothing: (),
}
