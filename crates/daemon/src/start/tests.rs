use iroh::SecretKey;
use project_runtime::operations::Operations;

#[test]
fn a_host_starting_on_a_data_directory_home_bound_first_keeps_home_identity()
-> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let home = Operations::home(dir.path())?.handle()?.owner;
    let id = SecretKey::from_bytes(&[3; 32]).public();
    let host = super::operations(dir.path(), id)?;
    assert_eq!(host.handle()?.owner, home);
    Ok(())
}
