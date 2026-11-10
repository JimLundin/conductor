//! Builder pattern for configuring and launching the Claude CLI process.
//!
//! This module provides [`ClaudeCliBuilder`] for constructing Claude CLI commands
//! with the correct flags for JSON streaming mode. The builder automatically configures:
//!
//! - JSON streaming input/output formats
//! - Non-interactive print mode
//! - Verbose output for proper streaming
//! - OAuth token and API key environment variables for authentication
//!

use crate::io::PermissionMode;
use crate::models::EffortLevel;
use std::path::PathBuf;
use std::process::Stdio;
use uuid::Uuid;

/// Set by a running Claude Code session for its children. Left in place, the
/// parent's session state leaks into the child CLI.
pub const PARENT_SESSION_VARS: &[&str] = &[
    "CLAUDECODE",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_SESSION_ATTENDED",
    "CLAUDE_CODE_MESSAGING_SOCKET",
    "CLAUDE_CODE_MESSAGING_TOKEN",
    "CLAUDE_CODE_EXECPATH",
    "CLAUDE_PID",
    "CLAUDE_EFFORT",
];

/// A settings file the CLI can load.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingSource {
    User,
    Project,
    Local,
}

impl SettingSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Project => "project",
            Self::Local => "local",
        }
    }
}

/// Builder for creating Claude CLI commands in JSON streaming mode
///
/// This builder automatically configures Claude to use:
/// - `--print` mode for non-interactive operation
/// - `--output-format stream-json` for streaming JSON responses
/// - `--input-format stream-json` for JSON input
#[derive(Debug, Clone)]
pub struct ClaudeCliBuilder {
    command: PathBuf,
    working_directory: Option<PathBuf>,
    prompt: Option<String>,
    debug: Option<String>,
    verbose: bool,
    dangerously_skip_permissions: bool,
    allowed_tools: Vec<String>,
    disallowed_tools: Vec<String>,
    mcp_config: Vec<String>,
    append_system_prompt: Option<String>,
    permission_mode: Option<PermissionMode>,
    continue_conversation: bool,
    resume: Option<String>,
    fork_session: bool,
    model: Option<String>,
    fallback_model: Option<String>,
    effort: Option<EffortLevel>,
    settings: Option<String>,
    setting_sources: Option<Vec<SettingSource>>,
    add_dir: Vec<PathBuf>,
    ide: bool,
    strict_mcp_config: bool,
    session_id: Option<Uuid>,
    oauth_token: Option<String>,
    api_key: Option<String>,
    /// Tool for handling permission prompts (e.g., "stdio" for bidirectional control)
    permission_prompt_tool: Option<String>,
    /// Maximum number of tokens for extended thinking
    max_thinking_tokens: Option<u32>,
}

impl Default for ClaudeCliBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl ClaudeCliBuilder {
    /// Create a new Claude CLI builder with JSON streaming mode pre-configured
    pub fn new() -> Self {
        Self {
            command: PathBuf::from("claude"),
            working_directory: None,
            prompt: None,
            debug: None,
            verbose: false,
            dangerously_skip_permissions: false,
            allowed_tools: Vec::new(),
            disallowed_tools: Vec::new(),
            mcp_config: Vec::new(),
            append_system_prompt: None,
            permission_mode: None,
            continue_conversation: false,
            resume: None,
            fork_session: false,
            model: None,
            fallback_model: None,
            effort: None,
            settings: None,
            setting_sources: None,
            add_dir: Vec::new(),
            ide: false,
            strict_mcp_config: false,
            session_id: None,
            oauth_token: None,
            api_key: None,
            permission_prompt_tool: None,
            max_thinking_tokens: None,
        }
    }

    /// Set custom path to Claude binary
    pub fn command<P: Into<PathBuf>>(mut self, path: P) -> Self {
        self.command = path.into();
        self
    }

    /// Set the working directory for the Claude process.
    pub fn working_directory<P: Into<PathBuf>>(mut self, path: P) -> Self {
        self.working_directory = Some(path.into());
        self
    }

    /// Set the prompt for Claude
    pub fn prompt<S: Into<String>>(mut self, prompt: S) -> Self {
        self.prompt = Some(prompt.into());
        self
    }

    /// Enable debug mode with optional filter
    pub fn debug<S: Into<String>>(mut self, filter: Option<S>) -> Self {
        self.debug = filter.map(|s| s.into());
        self
    }

    /// Enable verbose mode
    pub fn verbose(mut self, verbose: bool) -> Self {
        self.verbose = verbose;
        self
    }

    /// Skip all permission checks (dangerous!)
    pub fn dangerously_skip_permissions(mut self, skip: bool) -> Self {
        self.dangerously_skip_permissions = skip;
        self
    }

    /// Add allowed tools
    pub fn allowed_tools<I, S>(mut self, tools: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.allowed_tools
            .extend(tools.into_iter().map(|s| s.into()));
        self
    }

    /// Add disallowed tools
    pub fn disallowed_tools<I, S>(mut self, tools: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.disallowed_tools
            .extend(tools.into_iter().map(|s| s.into()));
        self
    }

    /// Add MCP configuration
    pub fn mcp_config<I, S>(mut self, configs: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.mcp_config
            .extend(configs.into_iter().map(|s| s.into()));
        self
    }

    /// Append a system prompt
    pub fn append_system_prompt<S: Into<String>>(mut self, prompt: S) -> Self {
        self.append_system_prompt = Some(prompt.into());
        self
    }

    /// Set permission mode
    pub fn permission_mode(mut self, mode: PermissionMode) -> Self {
        self.permission_mode = Some(mode);
        self
    }

    /// Continue the most recent conversation
    pub fn continue_conversation(mut self, continue_conv: bool) -> Self {
        self.continue_conversation = continue_conv;
        self
    }

    /// Resume a specific conversation
    pub fn resume<S: Into<String>>(mut self, session_id: Option<S>) -> Self {
        self.resume = session_id.map(|s| s.into());
        self
    }

    /// When resuming (or continuing), create a new session ID instead of
    /// reusing the source session's — i.e. fork. The CLI only accepts
    /// `--session-id` alongside `--resume`/`--continue` when this is set.
    ///
    /// Prefer [`fork_from`](Self::fork_from), which assembles the whole
    /// combination.
    pub fn fork_session(mut self, fork: bool) -> Self {
        self.fork_session = fork;
        self
    }

    /// Fork an existing session: resume `source_session_id`'s full history
    /// into a **new** session, leaving the source untouched.
    ///
    /// Assembles `--resume=<source> --fork-session --session-id <new>`,
    /// generating a fresh UUID for the fork (override it by chaining
    /// [`session_id`](Self::session_id)). The forked session starts with the
    /// source's entire history — the Claude CLI has no headless
    /// fork-at-a-point cut; for that semantic see `codex-codes`'
    /// `thread_fork` with `last_turn_id`.
    ///
    /// ```no_run
    /// # use claude_codes::ClaudeCliBuilder;
    /// let builder = ClaudeCliBuilder::new()
    ///     .fork_from("0b8fa762-6c48-4f3b-a3a1-0347f96a52bc")
    ///     .prompt("Continue, but try the other approach");
    /// ```
    pub fn fork_from<S: Into<String>>(mut self, source_session_id: S) -> Self {
        self.resume = Some(source_session_id.into());
        self.fork_session = true;
        self
    }

    /// Set the model to use
    pub fn model<S: Into<String>>(mut self, model: S) -> Self {
        self.model = Some(model.into());
        self
    }

    /// Set fallback model for overload situations
    pub fn fallback_model<S: Into<String>>(mut self, model: S) -> Self {
        self.fallback_model = Some(model.into());
        self
    }

    /// Set the session's effort level (`--effort`). The CLI may cap or
    /// downgrade it for the model or org; `system/init`'s `effort` reports the
    /// level actually used. Change it mid-session with an
    /// `apply_flag_settings` control request (see
    /// [`ApplyFlagSettingsRequest`](crate::io::ApplyFlagSettingsRequest)).
    pub fn effort(mut self, level: EffortLevel) -> Self {
        self.effort = Some(level);
        self
    }

    /// Set maximum number of tokens for extended thinking
    pub fn max_thinking_tokens(mut self, tokens: u32) -> Self {
        self.max_thinking_tokens = Some(tokens);
        self
    }

    /// Which settings files the CLI loads (`--setting-sources`). Unset, the
    /// CLI loads user, project and local settings.
    pub fn setting_sources(mut self, sources: impl IntoIterator<Item = SettingSource>) -> Self {
        self.setting_sources = Some(sources.into_iter().collect());
        self
    }

    /// Load settings from file or JSON
    pub fn settings<S: Into<String>>(mut self, settings: S) -> Self {
        self.settings = Some(settings.into());
        self
    }

    /// Add directories for tool access
    pub fn add_directories<I, P>(mut self, dirs: I) -> Self
    where
        I: IntoIterator<Item = P>,
        P: Into<PathBuf>,
    {
        self.add_dir.extend(dirs.into_iter().map(|p| p.into()));
        self
    }

    /// Automatically connect to IDE
    pub fn ide(mut self, ide: bool) -> Self {
        self.ide = ide;
        self
    }

    /// Use only MCP servers from config
    pub fn strict_mcp_config(mut self, strict: bool) -> Self {
        self.strict_mcp_config = strict;
        self
    }

    /// Set a specific session ID (must be a UUID)
    pub fn session_id(mut self, id: Uuid) -> Self {
        self.session_id = Some(id);
        self
    }

    /// Set OAuth token for authentication (must start with "sk-ant-oat")
    pub fn oauth_token<S: Into<String>>(mut self, token: S) -> Self {
        let token_str = token.into();
        if !token_str.starts_with("sk-ant-oat") {
            eprintln!("Warning: OAuth token should start with 'sk-ant-oat'");
        }
        self.oauth_token = Some(token_str);
        self
    }

    /// Set API key for authentication (must start with "sk-ant-api")
    pub fn api_key<S: Into<String>>(mut self, key: S) -> Self {
        let key_str = key.into();
        if !key_str.starts_with("sk-ant-api") {
            eprintln!("Warning: API key should start with 'sk-ant-api'");
        }
        self.api_key = Some(key_str);
        self
    }

    /// Enable bidirectional tool permission protocol via stdio
    ///
    /// When enabled, Claude CLI will send permission requests via stdout
    /// and expect responses via stdin. Use "stdio" for standard I/O based
    /// permission handling.
    ///
    /// # Example
    /// ```
    /// use claude_codes::ClaudeCliBuilder;
    ///
    /// let builder = ClaudeCliBuilder::new()
    ///     .permission_prompt_tool("stdio")
    ///     .model("sonnet");
    /// ```
    pub fn permission_prompt_tool<S: Into<String>>(mut self, tool: S) -> Self {
        self.permission_prompt_tool = Some(tool.into());
        self
    }

    /// Build the command arguments (always includes JSON streaming flags)
    fn build_args(&self) -> Vec<String> {
        // Always add JSON streaming mode flags
        // Note: --print with stream-json requires --verbose
        let mut args = vec![
            "--print".to_string(),
            "--verbose".to_string(),
            "--output-format".to_string(),
            "stream-json".to_string(),
            "--input-format".to_string(),
            "stream-json".to_string(),
        ];

        if let Some(ref debug) = self.debug {
            args.push("--debug".to_string());
            if !debug.is_empty() {
                args.push(debug.clone());
            }
        }

        if self.dangerously_skip_permissions {
            args.push("--dangerously-skip-permissions".to_string());
        }

        if !self.allowed_tools.is_empty() {
            args.push("--allowed-tools".to_string());
            args.extend(self.allowed_tools.clone());
        }

        if !self.disallowed_tools.is_empty() {
            args.push("--disallowed-tools".to_string());
            args.extend(self.disallowed_tools.clone());
        }

        if !self.mcp_config.is_empty() {
            args.push("--mcp-config".to_string());
            args.extend(self.mcp_config.clone());
        }

        if let Some(ref prompt) = self.append_system_prompt {
            args.push("--append-system-prompt".to_string());
            args.push(prompt.clone());
        }

        if let Some(ref mode) = self.permission_mode {
            args.push("--permission-mode".to_string());
            args.push(mode.as_str().to_string());
        }

        if self.continue_conversation {
            args.push("--continue".to_string());
        }

        // One argument, so an id that starts with `-` is not read as a flag.
        if let Some(ref session) = self.resume {
            args.push(format!("--resume={session}"));
        }

        if self.fork_session {
            args.push("--fork-session".to_string());
        }

        if let Some(ref model) = self.model {
            args.push("--model".to_string());
            args.push(model.clone());
        }

        if let Some(ref model) = self.fallback_model {
            args.push("--fallback-model".to_string());
            args.push(model.clone());
        }

        if let Some(ref level) = self.effort {
            args.push("--effort".to_string());
            args.push(level.to_string());
        }

        if let Some(tokens) = self.max_thinking_tokens {
            args.push("--max-thinking-tokens".to_string());
            args.push(tokens.to_string());
        }

        if let Some(ref settings) = self.settings {
            args.push("--settings".to_string());
            args.push(settings.clone());
        }

        if let Some(ref sources) = self.setting_sources {
            let names: Vec<&str> = sources.iter().map(|s| s.as_str()).collect();
            args.push("--setting-sources".to_string());
            args.push(names.join(","));
        }

        if !self.add_dir.is_empty() {
            args.push("--add-dir".to_string());
            for dir in &self.add_dir {
                args.push(dir.to_string_lossy().to_string());
            }
        }

        if self.ide {
            args.push("--ide".to_string());
        }

        if self.strict_mcp_config {
            args.push("--strict-mcp-config".to_string());
        }

        if let Some(ref tool) = self.permission_prompt_tool {
            args.push("--permission-prompt-tool".to_string());
            args.push(tool.clone());
        }

        // --session-id is only legal alongside --resume/--continue when
        // --fork-session is set (it names the fork); otherwise emit it only
        // for fresh sessions. Either way a UUID is generated when unset so
        // the session id is known before spawn.
        if (self.resume.is_none() && !self.continue_conversation) || self.fork_session {
            args.push("--session-id".to_string());
            let session_uuid = self.session_id.unwrap_or_else(Uuid::new_v4);
            args.push(session_uuid.to_string());
        }

        // Add prompt as the last argument if provided
        if let Some(ref prompt) = self.prompt {
            args.push(prompt.clone());
        }

        args
    }

    /// The command with piped stdio, ready to spawn. `kill_on_drop` is set, and
    /// [`PARENT_SESSION_VARS`] are removed so the CLI starts as its own session
    /// even when this process runs inside another Claude Code session.
    pub fn build_command(self) -> tokio::process::Command {
        let args = self.build_args();
        let mut cmd = tokio::process::Command::new(&self.command);
        cmd.args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        for var in PARENT_SESSION_VARS {
            cmd.env_remove(var);
        }

        if let Some(ref dir) = self.working_directory {
            cmd.current_dir(dir);
        }

        if let Some(ref token) = self.oauth_token {
            cmd.env("CLAUDE_CODE_OAUTH_TOKEN", token);
        }

        if let Some(ref key) = self.api_key {
            cmd.env("ANTHROPIC_API_KEY", key);
        }

        crate::process::configure_no_window(cmd.as_std_mut());
        cmd
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_working_directory() {
        let builder = ClaudeCliBuilder::new().working_directory("workspace");
        assert_eq!(builder.working_directory, Some(PathBuf::from("workspace")));
    }

    #[test]
    fn test_streaming_flags_always_present() {
        let builder = ClaudeCliBuilder::new();
        let args = builder.build_args();

        // Verify all streaming flags are present by default
        assert!(args.contains(&"--print".to_string()));
        assert!(args.contains(&"--verbose".to_string())); // Required for --print with stream-json
        assert!(args.contains(&"--output-format".to_string()));
        assert!(args.contains(&"stream-json".to_string()));
        assert!(args.contains(&"--input-format".to_string()));
    }

    #[test]
    fn test_with_prompt() {
        let builder = ClaudeCliBuilder::new().prompt("Hello, Claude!");
        let args = builder.build_args();

        assert_eq!(args.last().unwrap(), "Hello, Claude!");
    }

    #[test]
    fn test_with_model() {
        let builder = ClaudeCliBuilder::new()
            .model("sonnet")
            .fallback_model("opus");
        let args = builder.build_args();

        assert!(args.contains(&"--model".to_string()));
        assert!(args.contains(&"sonnet".to_string()));
        assert!(args.contains(&"--fallback-model".to_string()));
        assert!(args.contains(&"opus".to_string()));
    }

    #[test]
    fn test_with_effort() {
        let args = ClaudeCliBuilder::new()
            .effort(EffortLevel::Xhigh)
            .build_args();
        let i = args.iter().position(|a| a == "--effort").expect("--effort");
        assert_eq!(args[i + 1], "xhigh");
        assert!(!ClaudeCliBuilder::new()
            .build_args()
            .contains(&"--effort".to_string()));
    }

    #[test]
    fn test_with_debug() {
        let builder = ClaudeCliBuilder::new().debug(Some("api"));
        let args = builder.build_args();

        assert!(args.contains(&"--debug".to_string()));
        assert!(args.contains(&"api".to_string()));
    }

    #[test]
    fn test_with_oauth_token() {
        let valid_token = "sk-ant-oat-123456789";
        let builder = ClaudeCliBuilder::new().oauth_token(valid_token);

        // OAuth token is set as env var, not in args
        let args = builder.clone().build_args();
        assert!(!args.contains(&valid_token.to_string()));

        // Verify it's stored in the builder
        assert_eq!(builder.oauth_token, Some(valid_token.to_string()));
    }

    #[test]
    fn test_oauth_token_validation() {
        // Test with invalid prefix (should print warning but still accept)
        let invalid_token = "invalid-token-123";
        let builder = ClaudeCliBuilder::new().oauth_token(invalid_token);
        assert_eq!(builder.oauth_token, Some(invalid_token.to_string()));
    }

    #[test]
    fn test_with_api_key() {
        let valid_key = "sk-ant-api-987654321";
        let builder = ClaudeCliBuilder::new().api_key(valid_key);

        // API key is set as env var, not in args
        let args = builder.clone().build_args();
        assert!(!args.contains(&valid_key.to_string()));

        // Verify it's stored in the builder
        assert_eq!(builder.api_key, Some(valid_key.to_string()));
    }

    #[test]
    fn test_api_key_validation() {
        // Test with invalid prefix (should print warning but still accept)
        let invalid_key = "invalid-api-key";
        let builder = ClaudeCliBuilder::new().api_key(invalid_key);
        assert_eq!(builder.api_key, Some(invalid_key.to_string()));
    }

    #[test]
    fn test_both_auth_methods() {
        let oauth = "sk-ant-oat-123";
        let api_key = "sk-ant-api-456";
        let builder = ClaudeCliBuilder::new().oauth_token(oauth).api_key(api_key);

        assert_eq!(builder.oauth_token, Some(oauth.to_string()));
        assert_eq!(builder.api_key, Some(api_key.to_string()));
    }

    #[test]
    fn test_permission_prompt_tool() {
        let builder = ClaudeCliBuilder::new().permission_prompt_tool("stdio");
        let args = builder.build_args();

        assert!(args.contains(&"--permission-prompt-tool".to_string()));
        assert!(args.contains(&"stdio".to_string()));
    }

    #[test]
    fn test_permission_prompt_tool_not_present_by_default() {
        let builder = ClaudeCliBuilder::new();
        let args = builder.build_args();

        assert!(!args.contains(&"--permission-prompt-tool".to_string()));
    }

    #[test]
    fn test_session_id_present_for_new_session() {
        let builder = ClaudeCliBuilder::new();
        let args = builder.build_args();

        assert!(
            args.contains(&"--session-id".to_string()),
            "New sessions should have --session-id"
        );
    }

    #[test]
    fn test_session_id_not_present_with_resume() {
        // When resuming a session, --session-id should NOT be added
        // (Claude CLI rejects --session-id + --resume without --fork-session)
        let builder = ClaudeCliBuilder::new().resume(Some("existing-uuid".to_string()));
        let args = builder.build_args();

        assert!(args.contains(&"--resume=existing-uuid".to_string()));
        assert!(
            !args.contains(&"--session-id".to_string()),
            "--session-id should NOT be present when resuming"
        );
    }

    #[test]
    fn test_setting_sources() {
        let args = ClaudeCliBuilder::new()
            .setting_sources([SettingSource::Project, SettingSource::Local])
            .build_args();
        let i = args.iter().position(|a| a == "--setting-sources").unwrap();
        assert_eq!(args[i + 1], "project,local");
    }

    #[test]
    fn test_session_id_not_present_with_continue() {
        // When continuing a session, --session-id should NOT be added
        let builder = ClaudeCliBuilder::new().continue_conversation(true);
        let args = builder.build_args();

        assert!(
            args.contains(&"--continue".to_string()),
            "Should have --continue flag"
        );
        assert!(
            !args.contains(&"--session-id".to_string()),
            "--session-id should NOT be present when continuing"
        );
    }

    #[test]
    fn test_fork_from_assembles_resume_fork_and_new_session_id() {
        let new_id = Uuid::new_v4();
        let args = ClaudeCliBuilder::new()
            .fork_from("source-uuid")
            .session_id(new_id)
            .build_args();

        assert!(args.contains(&"--resume=source-uuid".to_string()));
        assert!(args.contains(&"--fork-session".to_string()));
        let sid_pos = args.iter().position(|a| a == "--session-id").unwrap();
        assert_eq!(args[sid_pos + 1], new_id.to_string());
    }

    #[test]
    fn test_fork_from_generates_session_id_when_unset() {
        let args = ClaudeCliBuilder::new()
            .fork_from("source-uuid")
            .build_args();

        assert!(args.contains(&"--fork-session".to_string()));
        let sid_pos = args.iter().position(|a| a == "--session-id").unwrap();
        assert!(
            Uuid::parse_str(&args[sid_pos + 1]).is_ok(),
            "generated fork session id should be a UUID"
        );
    }

    #[test]
    fn test_fork_session_with_continue_emits_session_id() {
        let args = ClaudeCliBuilder::new()
            .continue_conversation(true)
            .fork_session(true)
            .build_args();

        assert!(args.contains(&"--continue".to_string()));
        assert!(args.contains(&"--fork-session".to_string()));
        assert!(args.contains(&"--session-id".to_string()));
    }
}
