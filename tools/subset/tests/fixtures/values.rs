fn values(flag: Flag) -> Flag {
    let yes = true;
    let pair = (1, 2);
    let unit = ();
    let list = [1, 2];
    let typed: u32 = 1;
    match flag {
        Flag(false) => Flag(true),
        Flag(true) => Flag(false),
    }
}

fn shapes(pair: (u32, u32), list: &[u32], r: &u32) -> u32 {
    match pair {
        (0, ..) => 0,
        (1..=9, _) => 1,
        _ => 2,
    }
}
