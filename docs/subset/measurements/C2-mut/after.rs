async fn run(live: &mut Live, scratch: &Path) -> anyhow::Result<()> {
    let t = turn(live, APPROVE_PROMPT, Policy::Approve).await?;
    let approve = Check::new("1 approve", writes_once(&t), format!("permissions={:?}", t.permissions))?;
    let t = turn(live, DENY_PROMPT, Policy::Deny("no")).await?;
    let deny = Check::new("2 deny", writes_once(&t), format!("permissions={:?}", t.permissions))?;
    match Vec::from([approve, deny]).iter().filter_map(Check::failed).count() {
        0 => Ok(()),
        failed => Err(anyhow::Error::msg(format!("{failed} step(s) failed"))),
    }
}

impl Check {
    fn new(step: &str, verdict: Verdict, detail: String) -> std::io::Result<Check> {
        writeln!(std::io::stdout(), "{} {step}: {detail}", verdict.label())?;
        Ok(Check { verdict })
    }

    fn failed(&self) -> Option<&Check> {
        match self.verdict {
            Verdict::Pass => None,
            Verdict::Fail => Some(self),
        }
    }
}
