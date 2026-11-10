fn flow(x: Option<u32>, n: u32) -> u32 {
    if n {
        return 1;
    } else if n {
        return 2;
    }
    if let Some(v) = x {
        return v;
    }
    while n {
        break;
    }
    while let Some(v) = x {
        continue;
    }
    for v in x {}
    loop {}
    match x {
        Some(v) if v => 0,
        _ => 1,
    }
}
