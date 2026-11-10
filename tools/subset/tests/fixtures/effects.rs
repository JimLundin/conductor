fn effects(future: Future) -> u32 {
    let ready = async { 1 };
    let value = future.await;
    unsafe { value }
}
