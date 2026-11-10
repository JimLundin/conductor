fn closures(items: Vec<u32>) -> Vec<u32> {
    let double = |x| x;
    std::thread::spawn(|| items);
    items.into_iter().map(|x| x).collect()
}
