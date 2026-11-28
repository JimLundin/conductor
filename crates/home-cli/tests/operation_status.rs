use std::process::Command;

#[test]
fn operation_status_on_a_fresh_data_directory_says_the_operation_is_not_this_host_s()
-> std::io::Result<()> {
    let data = std::env::temp_dir()
        .join(format!("home-cli-operation-status-{}", std::process::id()));
    let handle = format!("{}/01ARZ3NDEKTSV4RRFFQ69G5FAV", "07".repeat(32));
    let answered = Command::new(env!("CARGO_BIN_EXE_home"))
        .args(["project", "operation-status", &handle, "--data-dir"])
        .arg(&data)
        .output();
    let _removed = std::fs::remove_dir_all(&data);
    let ran = answered?;
    assert_eq!(
        String::from_utf8_lossy(&ran.stdout),
        "home: the operation belongs to another host\n"
    );
    assert_eq!(ran.status.code(), Some(1));
    Ok(())
}
