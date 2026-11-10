async fn run(live: &mut Live, scratch: &Path) -> anyhow::Result<()> {
    let mut failures = 0;
    let mut report = |step: &str, ok: bool, detail: String| {
        println!("{} {step}: {detail}", if ok { "PASS" } else { "FAIL" });
        if !ok {
            failures += 1;
        }
    };
    let t = turn(live, APPROVE_PROMPT, Policy::Approve).await?;
    report("1 approve", t.permissions == ["Write"], format!("permissions={:?}", t.permissions));
    let t = turn(live, DENY_PROMPT, Policy::Deny("no")).await?;
    report("2 deny", t.permissions == ["Write"], format!("permissions={:?}", t.permissions));
    if failures > 0 {
        bail!("{failures} step(s) failed");
    }
    Ok(())
}
