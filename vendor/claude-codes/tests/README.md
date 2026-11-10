# Tests

The tests here parse recorded CLI output. None of them start the CLI. The live
check is `cargo run --bin s1` in `crates/agents`.

## Corpus tests

Real Claude Code transcript JSONL coverage is opt-in because the corpus lives
outside the repository and can be large:

```bash
CLAUDE_CODES_CORPUS_DIR="$HOME/.claude/projects" \
  cargo test --features corpus-test parse_configured_claude_transcript_corpus -- --nocapture
```

The corpus test recursively scans every `.jsonl` file under the configured
directory, skips malformed JSON lines, parses valid lines with
`ClaudeOutput::parse_json_tolerant()`, prints counts by message type, and fails
on any valid JSON line that cannot be represented by the crate.
