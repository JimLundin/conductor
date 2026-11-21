//! How a `claude` process starts, as data, and what its first answer
//! decides: its command line, its environment and whether its version
//! runs.

use agents::pin::Cli;
use agents::{Config, HomeKind, Invocation, Note, Profile, Reported, Var};
use proto::{Effort, Provider};
use serde_json::Value;

use crate::prelude::*;
use crate::{
    ClaudeCredential, ClaudeEvent, MessageId, PermissionMode, SessionId,
    UnsupportedVersion, check_version,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingSource {
    User,
    Project,
    Local,
}

impl SettingSource {
    const fn wire(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Project => "project",
            Self::Local => "local",
        }
    }
}

/// Which CLI versions a session agrees to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionPolicy {
    /// Refuse a CLI outside the supported range.
    SupportedOnly,
    /// Run it anyway, with a warning.
    AllowUntested,
}

/// Native conversation history to load. A fork is inclusive of `through`.
///
/// Its target transcript is created lazily on the first real user turn, so
/// persist this recipe until then; `id()` alone does not prove persistence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resume {
    Fresh,
    Session(SessionId),
    Fork {
        session: SessionId,
        through: MessageId,
        target: SessionId,
    },
}

impl Resume {
    /// The session this run continues as; a fresh one takes a new id.
    #[must_use]
    pub const fn target(&self) -> Option<&SessionId> {
        match self {
            Self::Fresh => None,
            Self::Session(id) | Self::Fork { target: id, .. } => Some(id),
        }
    }

    /// The session whose transcript `reason`, why the CLI exited at start,
    /// says is missing. Recovery may replay a persisted fork recipe only for
    /// that exact reason about the requested resume id.
    #[must_use]
    pub fn missing(&self, reason: &str) -> Option<SessionId> {
        match self {
            Self::Session(id)
                if reason
                    == format!(
                        "No conversation found with session ID: {}",
                        id.0
                    ) =>
            {
                Some(id.clone())
            }
            Self::Fresh | Self::Session(_) | Self::Fork { .. } => None,
        }
    }
}

/// Whether Claude's command sandbox can run on this host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sandbox {
    Available,
    Missing,
}

/// What Claude's sandbox needs on `PATH` on Linux and WSL2.
pub const SANDBOX_TOOLS: &[&str] = &["bwrap", "socat"];

/// Whether Claude's command sandbox runs on this OS and kernel with
/// all of [`SANDBOX_TOOLS`] on `PATH`. WSL1 lacks the namespaces it needs.
#[must_use]
pub fn sandbox(os: &str, tools: bool, kernel_release: &str) -> Sandbox {
    let release = kernel_release.to_ascii_lowercase();
    let wsl1 = release.contains("microsoft")
        && !release.contains("microsoft-standard")
        && !release.contains("wsl2");
    match (os, tools, wsl1) {
        ("macos", _, _) | ("linux", true, false) => Sandbox::Available,
        (_, _, _) => Sandbox::Missing,
    }
}

/// The variables a parent Claude Code session sets. They are removed so the CLI
/// starts as its own session even when Conductor runs inside one.
const PARENT_SESSION_VARS: &[&str] = &[
    "CLAUDECODE",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_PROJECT_DIR_NAME",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_SESSION_ATTENDED",
    "CLAUDE_CODE_MESSAGING_SOCKET",
    "CLAUDE_CODE_MESSAGING_TOKEN",
    "CLAUDE_CODE_EXECPATH",
    "CLAUDE_PID",
    "CLAUDE_EFFORT",
];

/// What the host's own environment may carry that would take a private home
/// off its own sign-in: credentials, which take precedence over it, the
/// cloud providers and another endpoint, and another home.
const INHERITED_LOGIN: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "CLAUDE_CODE_OAUTH_TOKEN",
    "CLAUDE_CODE_USE_BEDROCK",
    "CLAUDE_CODE_USE_VERTEX",
    "CLAUDE_CODE_USE_FOUNDRY",
    "ANTHROPIC_BASE_URL",
    "CLAUDE_CONFIG_DIR",
];

fn removed(names: &[&str]) -> Vec<Var> {
    names.iter().map(|name| Var::remove(name)).collect()
}

/// What every run of `claude` gets: its home, and its own updates off,
/// since the host installs the version it runs.
#[must_use]
pub fn cli(home: HomeKind) -> Invocation {
    let own_home = match home {
        HomeKind::User => Vec::new(),
        HomeKind::Private => [
            removed(INHERITED_LOGIN),
            Vec::from([Var::Home(String::from("CLAUDE_CONFIG_DIR"))]),
        ]
        .concat(),
    };
    Invocation::of(
        [
            Vec::from([Var::set("DISABLE_UPDATES", "1")]),
            own_home,
            removed(PARENT_SESSION_VARS),
        ]
        .concat(),
        &[],
    )
}

/// `claude auth status`, which succeeds when `home` is signed in.
#[must_use]
pub fn auth_status(home: HomeKind) -> Invocation {
    Invocation::of(cli(home).env, &["auth", "status"])
}

/// The subscription sign-in, `claude auth login --claudeai`. It stores the
/// credential in its home itself.
#[must_use]
pub fn login(home: HomeKind) -> Invocation {
    Invocation::of(cli(home).env, &["auth", "login", "--claudeai"])
}

/// Claude Code as the host runs it. Its settings keep transcripts past
/// Claude Code's default 30-day sweep; Conductor deletes a transcript with
/// its session (§6.2).
#[must_use]
pub fn profile() -> Profile {
    Profile {
        provider: Provider::Claude,
        info: crate::info(),
        pin: Cli::Claude,
        instructions_file: String::from("CLAUDE.md"),
        config: Config {
            file: String::from("settings.json"),
            contents: String::from("{\n  \"cleanupPeriodDays\": 36500\n}\n"),
        },
        version: Invocation::of(cli(HomeKind::Private).env, &["--version"]),
        status: auth_status(HomeKind::Private),
        account: Reported::Status,
        sign_in: crate::sign_in(),
    }
}

/// What a session's `claude` runs with.
#[derive(Debug, Clone)]
pub struct Launch {
    pub home: HomeKind,
    /// `None` leaves the home's own sign-in.
    pub credential: Option<ClaudeCredential>,
    /// Conductor's stable session ULID isolates its transcript directory.
    pub project_name: Option<String>,
    pub permission_mode: PermissionMode,
    pub resume: Resume,
    /// The session id the CLI runs as: the resume target, or a new one.
    pub id: SessionId,
    pub model: Option<String>,
    /// `None` leaves the model's default effort.
    pub effort: Option<Effort>,
    /// `None` leaves the CLI default, which loads user, project and local
    /// settings.
    pub setting_sources: Option<Vec<SettingSource>>,
    /// Appended to the CLI's system prompt.
    pub instructions: Option<String>,
    pub sandbox: Sandbox,
}

/// The session's `claude --print` over stream-json, with the control
/// protocol on stdio.
#[must_use]
pub fn session(launch: &Launch) -> Invocation {
    // `TodoWrite`, the checklist the host keeps as tasks, rather than the
    // task tools that would take its place (PROTOCOL.md).
    let tasks = Vec::from([
        Var::set("CLAUDE_CODE_ENABLE_TODO_TOOLS", "1"),
        Var::set("CLAUDE_CODE_ENABLE_TASKS", "0"),
    ]);
    let env = [
        cli(launch.home).env,
        launch
            .project_name
            .iter()
            .map(|name| Var::set("CLAUDE_CODE_PROJECT_DIR_NAME", name))
            .collect(),
        gateway(launch.credential.as_ref()),
        tasks,
        scrub(launch.credential.as_ref(), launch.sandbox),
    ]
    .concat();
    Invocation {
        env,
        args: session_args(launch),
    }
}

fn gateway(credential: Option<&ClaudeCredential>) -> Vec<Var> {
    match credential {
        Some(ClaudeCredential::Gateway { base_url, token }) => Vec::from([
            Var::set("ANTHROPIC_BASE_URL", base_url),
            Var::set("ANTHROPIC_AUTH_TOKEN", token),
        ]),
        None => Vec::new(),
    }
}

/// The scrub keeps credentials out of the commands, hooks and MCP servers
/// Claude starts, and Claude itself still uses them. On Linux it runs every
/// command in Claude's sandbox, so it is set only where that runs.
fn scrub(credential: Option<&ClaudeCredential>, sandbox: Sandbox) -> Vec<Var> {
    match (credential, sandbox) {
        (Some(ClaudeCredential::Gateway { .. }), Sandbox::Available) => {
            Vec::from([Var::set("CLAUDE_CODE_SUBPROCESS_ENV_SCRUB", "1")])
        }
        (Some(ClaudeCredential::Gateway { .. }), Sandbox::Missing)
        | (None, _) => Vec::new(),
    }
}

fn strings(args: &[&str]) -> Vec<String> {
    args.iter().copied().map(String::from).collect()
}

fn session_args(launch: &Launch) -> Vec<String> {
    let model = launch
        .model
        .iter()
        .flat_map(|model| [String::from("--model"), model.clone()]);
    let effort = launch
        .effort
        .iter()
        .flat_map(|effort| strings(&["--effort", effort.wire()]));
    let sources = launch.setting_sources.iter().flat_map(|sources| {
        let names = sources
            .iter()
            .map(|source| source.wire())
            .collect::<Vec<&str>>()
            .join(",");
        [String::from("--setting-sources"), names]
    });
    let instructions = launch
        .instructions
        .iter()
        .map(|text| format!("--append-system-prompt={text}"));
    [
        strings(&[
            "--print",
            "--verbose",
            "--output-format",
            "stream-json",
            "--input-format",
            "stream-json",
            "--permission-mode",
            launch.permission_mode.wire(),
        ]),
        resume_args(&launch.resume, &launch.id),
        model
            .chain(effort)
            .chain(sources)
            .chain(instructions)
            .collect(),
        // A hidden flag. The text arrives only where the CLI also sends
        // experimental betas, which `CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS`
        // turns off (PROTOCOL.md).
        strings(&[
            "--replay-user-messages",
            "--include-partial-messages",
            "--thinking-display",
            "summarized",
            "--permission-prompt-tool",
            "stdio",
        ]),
    ]
    .concat()
}

/// One argument each, so an id that starts with `-` is not read as a flag.
fn resume_args(resume: &Resume, id: &SessionId) -> Vec<String> {
    match resume {
        Resume::Fresh => strings(&["--session-id", &id.0]),
        Resume::Session(session) => {
            Vec::from([format!("--resume={}", session.0)])
        }
        Resume::Fork {
            session,
            through,
            target,
        } => Vec::from([
            format!("--resume={}", session.0),
            format!("--resume-session-at={}", through.0),
            String::from("--fork-session"),
            String::from("--session-id"),
            target.0.clone(),
        ]),
    }
}

/// Why a CLI exited before it answered `initialize`, if it said: the
/// `errors` of the first `result` among the `events` it printed.
#[must_use]
pub fn reported_refusal(events: Vec<ClaudeEvent>) -> Option<String> {
    events.into_iter().find_map(|event| match event {
        ClaudeEvent::TurnFinished { errors, .. } => {
            errors.first().map(|_| errors.join("; "))
        }
        _ => None,
    })
}

/// Whether the CLI that answered `initialize` runs under `policy`, with
/// the warning an untested one that runs is logged with.
///
/// # Errors
/// The CLI's version is unsupported or unreported and `policy` refuses it.
pub fn admit(
    initialize: Value,
    policy: VersionPolicy,
) -> Result<Option<Note>, UnsupportedVersion> {
    match (check_version(initialize), policy) {
        (Ok(()), _) => Ok(None),
        (
            Err(UnsupportedVersion::Outside { found, supported }),
            VersionPolicy::AllowUntested,
        ) => Ok(Some(Note::Untested {
            provider: Provider::Claude,
            found,
            supported,
        })),
        (Err(UnsupportedVersion::Unreported), VersionPolicy::AllowUntested) => {
            Ok(Some(Note::Unversioned {
                provider: Provider::Claude,
            }))
        }
        (Err(unsupported), VersionPolicy::SupportedOnly) => Err(unsupported),
    }
}

/// The warning a gateway credential gets when Claude's commands run
/// unsandboxed, since they then inherit its token.
#[must_use]
pub const fn unprotected(
    credential: Option<&ClaudeCredential>,
    sandbox: Sandbox,
) -> Option<Note> {
    match (credential, sandbox) {
        (Some(ClaudeCredential::Gateway { .. }), Sandbox::Missing) => {
            Some(Note::GatewayExposed {
                provider: Provider::Claude,
            })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use agents::{HomeKind, Invocation, Note, Var};
    use proto::{Effort, Provider};
    use serde_json::json;

    use super::{
        Launch, Resume, Sandbox, SettingSource, VersionPolicy, admit, cli,
        profile, reported_refusal, sandbox, session, unprotected,
    };
    use crate::{
        ClaudeCredential, ClaudeEvent, MessageId, PermissionMode, SessionId,
        UnsupportedVersion,
    };

    #[test]
    fn the_profile_runs_claude_in_its_private_home() {
        let profile = profile();
        let private = cli(HomeKind::Private).env;
        assert_eq!(
            (
                profile.provider,
                profile.config.file.as_str(),
                [profile.version, profile.status, profile.sign_in.run]
            ),
            (
                Provider::Claude,
                "settings.json",
                [
                    Invocation::of(private.clone(), &["--version"]),
                    Invocation::of(private.clone(), &["auth", "status"]),
                    Invocation::of(private, &["auth", "login", "--claudeai"]),
                ]
            )
        );
    }

    fn id(text: &str) -> SessionId {
        SessionId(String::from(text))
    }

    #[test]
    fn instructions_are_appended_to_the_system_prompt() {
        let told = Launch {
            instructions: Some(String::from("-- Be brief.")),
            ..launch(Resume::Fresh)
        };
        assert!(
            super::session_args(&told)
                .contains(&String::from("--append-system-prompt=-- Be brief."))
        );
        assert!(
            !super::session_args(&launch(Resume::Fresh))
                .iter()
                .any(|arg| arg.starts_with("--append-system-prompt"))
        );
    }

    fn launch(resume: Resume) -> Launch {
        Launch {
            instructions: None,
            home: HomeKind::User,
            credential: None,
            project_name: None,
            permission_mode: PermissionMode::Default,
            resume,
            id: id("new-session"),
            model: None,
            effort: None,
            setting_sources: None,
            sandbox: Sandbox::Missing,
        }
    }

    fn set(name: &str, value: &str) -> Var {
        Var::set(name, value)
    }

    #[test]
    fn only_the_requested_sessions_missing_transcript_is_missing() {
        let resume = Resume::Session(id("target"));
        let reason = |session: &str| {
            format!("No conversation found with session ID: {session}")
        };
        assert_eq!(
            [
                resume.missing(&reason("target")),
                resume.missing(&reason("another-session")),
                Resume::Fresh.missing(&reason("target")),
            ],
            [Some(id("target")), None, None]
        );
    }

    #[test]
    fn a_fork_resumes_through_its_message_as_the_chosen_session() {
        let fork = Resume::Fork {
            session: id("source"),
            through: MessageId(String::from("assistant-uuid")),
            target: id("new-session"),
        };
        let args = session(&launch(fork.clone())).args;
        assert_eq!(
            (&args[8..13], fork.target(), Resume::Fresh.target()),
            (
                &[
                    "--resume=source",
                    "--resume-session-at=assistant-uuid",
                    "--fork-session",
                    "--session-id",
                    "new-session",
                ]
                .map(String::from)[..],
                Some(&id("new-session")),
                None
            )
        );
    }

    #[test]
    fn a_fresh_session_takes_its_id_and_options_follow_the_resume() {
        let args = session(&Launch {
            model: Some(String::from("haiku")),
            effort: Some(Effort::XHigh),
            setting_sources: Some(Vec::from([
                SettingSource::User,
                SettingSource::Local,
            ])),
            ..launch(Resume::Fresh)
        })
        .args;
        assert_eq!(
            &args[8..16],
            [
                "--session-id",
                "new-session",
                "--model",
                "haiku",
                "--effort",
                "xhigh",
                "--setting-sources",
                "user,local",
            ]
        );
    }

    #[test]
    fn a_private_home_drops_inherited_sign_ins_and_a_gateway_scrubs_where_sandboxed()
     {
        let gateway = Launch {
            home: HomeKind::Private,
            credential: Some(ClaudeCredential::Gateway {
                base_url: String::from("https://gateway.example"),
                token: String::from("secret"),
            }),
            project_name: Some(String::from("01JC")),
            sandbox: Sandbox::Available,
            ..launch(Resume::Fresh)
        };
        let env = session(&gateway).env;
        let unscrubbed = session(&Launch {
            sandbox: Sandbox::Missing,
            ..gateway
        })
        .env;
        assert_eq!(
            (&env[..2], &env[8..10], &env[21..], unscrubbed.last(),),
            (
                &[
                    set("DISABLE_UPDATES", "1"),
                    Var::remove("ANTHROPIC_API_KEY")
                ][..],
                &[
                    Var::remove("CLAUDE_CONFIG_DIR"),
                    Var::Home(String::from("CLAUDE_CONFIG_DIR")),
                ][..],
                &[
                    set("CLAUDE_CODE_PROJECT_DIR_NAME", "01JC"),
                    set("ANTHROPIC_BASE_URL", "https://gateway.example"),
                    set("ANTHROPIC_AUTH_TOKEN", "secret"),
                    set("CLAUDE_CODE_ENABLE_TODO_TOOLS", "1"),
                    set("CLAUDE_CODE_ENABLE_TASKS", "0"),
                    set("CLAUDE_CODE_SUBPROCESS_ENV_SCRUB", "1"),
                ][..],
                Some(&set("CLAUDE_CODE_ENABLE_TASKS", "0")),
            )
        );
    }

    #[test]
    fn the_users_home_keeps_its_sign_in_and_signs_in_with_claudeai() {
        assert_eq!(
            super::login(HomeKind::User),
            Invocation {
                env: [
                    Vec::from([set("DISABLE_UPDATES", "1")]),
                    super::removed(super::PARENT_SESSION_VARS),
                ]
                .concat(),
                args: ["auth", "login", "--claudeai"].map(String::from).into(),
            }
        );
    }

    #[test]
    fn the_sandbox_runs_on_macos_and_on_linux_with_its_tools() {
        assert_eq!(
            [
                sandbox("macos", false, ""),
                sandbox("linux", true, ""),
                sandbox("linux", false, ""),
                sandbox("windows", true, ""),
            ],
            [
                Sandbox::Available,
                Sandbox::Available,
                Sandbox::Missing,
                Sandbox::Missing,
            ]
        );
    }

    #[test]
    fn the_first_result_with_errors_says_why_the_cli_refused() {
        let finished = |errors: &[&str]| ClaudeEvent::TurnFinished {
            subtype: String::from("error_during_execution"),
            outcome: proto::TurnOutcome::Failed {
                message: String::from("failed"),
            },
            terminal_reason: None,
            session_id: id("s"),
            errors: errors.iter().copied().map(String::from).collect(),
            last_message: None,
        };
        assert_eq!(
            (
                reported_refusal(Vec::from([
                    ClaudeEvent::StreamDelta {
                        text: String::from("…"),
                    },
                    finished(&[]),
                    finished(&["first", "second"]),
                ])),
                reported_refusal(Vec::new()),
            ),
            (Some(String::from("first; second")), None)
        );
    }

    #[test]
    fn an_untested_version_runs_only_when_the_policy_allows_it() {
        let old = || json!({ "claude_code_version": "1.0.0" });
        assert_eq!(
            [
                admit(old(), VersionPolicy::AllowUntested),
                admit(json!({}), VersionPolicy::AllowUntested),
                admit(json!({}), VersionPolicy::SupportedOnly),
                admit(
                    json!({ "claude_code_version": "2.1.316" }),
                    VersionPolicy::SupportedOnly
                ),
            ],
            [
                Ok(Some(Note::Untested {
                    provider: Provider::Claude,
                    found: String::from("1.0.0"),
                    supported: String::from("2.1.316 to 2.1.322"),
                })),
                Ok(Some(Note::Unversioned {
                    provider: Provider::Claude,
                })),
                Err(UnsupportedVersion::Unreported),
                Ok(None)
            ]
        );
    }

    #[test]
    fn only_an_unsandboxed_gateway_is_warned_about() {
        let gateway = ClaudeCredential::Gateway {
            base_url: String::from("https://gateway.example"),
            token: String::from("secret"),
        };
        assert_eq!(
            [
                unprotected(Some(&gateway), Sandbox::Missing),
                unprotected(Some(&gateway), Sandbox::Available),
                unprotected(None, Sandbox::Missing),
            ],
            [
                Some(Note::GatewayExposed {
                    provider: Provider::Claude
                }),
                None,
                None
            ]
        );
    }

    #[test]
    fn wsl1_cannot_sandbox_even_with_bubblewrap_and_socat() {
        assert_eq!(
            [
                sandbox("linux", true, "4.4.0-19041-Microsoft"),
                sandbox("linux", true, "5.15.167.4-microsoft-standard-WSL2"),
                sandbox("linux", true, "6.12.0-generic"),
                sandbox("linux", false, "6.12.0-generic"),
            ],
            [
                Sandbox::Missing,
                Sandbox::Available,
                Sandbox::Available,
                Sandbox::Missing,
            ]
        );
    }
}
