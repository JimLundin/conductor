use codex::{Effect, Input, Server};
use tracing::Level;

#[test]
fn malformed_codex_stdout_warns_without_the_raw_frame()
-> Result<(), Box<dyn std::error::Error>> {
    const MARKER: &str = "private-test-marker";
    let captured = tempfile::NamedTempFile::new()?;
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(Level::WARN)
        .with_ansi(false)
        .without_time()
        .with_writer(captured.reopen()?)
        .finish();
    tracing::subscriber::with_default(subscriber, || {
        let mut driver = Server::<()>::default();
        let effects: Vec<Effect<(), (), &'static str>> =
            driver.step(Input::Read(Ok(Some(String::from(MARKER)))));
        for effect in &effects {
            if let Effect::Log(note) = effect {
                runtime::log(note);
            }
        }
    });
    let text = std::fs::read_to_string(captured.path())?;
    assert!(text.contains("WARN"), "{text}");
    assert!(text.contains("agent"), "{text}");
    assert!(text.contains("provider=codex"), "{text}");
    assert!(text.contains("printed a line that is not JSON"), "{text}");
    assert!(text.contains("expected value at line 1 column 1"), "{text}");
    assert!(!text.contains(MARKER), "{text}");
    Ok(())
}
