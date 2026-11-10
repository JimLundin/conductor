fn log(name: &str) -> String {
    tracing::info!("x");
    info!("y");
    println!("z");
    assert_eq!(1, 1);
    format!("{name}")
}

#[cfg(test)]
mod tests {
    fn helper() {
        assert_eq!(1, 1);
        assert!(true);
    }
}

#[test]
fn top_level() {
    assert_eq!(1, 1);
}
