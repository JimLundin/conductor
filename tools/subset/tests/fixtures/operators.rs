fn ops(a: u32, b: u32, v: Vec<u32>) -> u32 {
    let sum = a + b;
    let both = a && b;
    let same = a == b;
    let not = !a;
    let neg = -a;
    let deref = *a;
    let cast = a as u64;
    let first = v[0];
    let span = a..b;
    a = b;
    a += b;
    sum
}
