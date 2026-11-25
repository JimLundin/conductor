use agents::Effort;
use agents::{Config, HomeKind, Invocation, Note, Profile, Reported, Var};
use serde_json::Value;

use crate::prelude::*;
use crate::{
    ClaudeCredential, ClaudeEvent, PermissionMode, SessionId,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionPolicy {
    SupportedOnly,
    AllowUntested,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resume {
    Fresh,
    Session(SessionId),
}

impl Resume {
    #[must_use]
    pub const fn target(&self) -> Option<&SessionId> {
        match self {
            Self::Fresh => None,
            Self::Session(id) => Some(id),
        }
    }

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
            Self::Fresh | Self::Session(_) => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sandbox {
    Available,
    Missing,
}

pub const SANDBOX_TOOLS: &[&str] = &["bwrap", "socat"];

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

const TRANSCRIPTS_KEPT_FOR_DAYS: u32 = 100 * 365;

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

#[must_use]
pub fn auth_status(home: HomeKind) -> Invocation {
    Invocation::of(cli(home).env, &["auth", "status"])
}

#[must_use]
pub fn login(home: HomeKind) -> Invocation {
    Invocation::of(cli(home).env, &["auth", "login", "--claudeai"])
}

#[must_use]
pub fn profile() -> Profile {
    Profile {
        provider: crate::PROVIDER,
        info: crate::info(),
        instructions_file: String::from("CLAUDE.md"),
        config: Config {
            file: String::from("settings.json"),
            contents: format!(
                "{{\n  \"cleanupPeriodDays\": {TRANSCRIPTS_KEPT_FOR_DAYS}\n}}\n"
            ),
        },
        version: Invocation::of(cli(HomeKind::Private).env, &["--version"]),
        status: auth_status(HomeKind::Private),
        account: Reported::Status,
        sign_in: crate::sign_in(),
    }
}

#[derive(Debug, Clone)]
pub struct Launch {
    pub home: HomeKind,
    pub credential: Option<ClaudeCredential>,
    pub project_name: Option<String>,
    pub permission_mode: PermissionMode,
    pub resume: Resume,
    pub id: SessionId,
    pub model: Option<String>,
    pub effort: Option<Effort>,
    pub setting_sources: Option<Vec<SettingSource>>,
    pub instructions: Option<String>,
    pub sandbox: Sandbox,
}

#[must_use]
pub fn session(launch: &Launch) -> Invocation {
    let checklist_tools = Vec::from([
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
        credential_env(launch.credential.as_ref()),
        checklist_tools,
        sandboxed_subprocess_scrub(launch.credential.as_ref(), launch.sandbox),
    ]
    .concat();
    Invocation {
        env,
        args: session_args(launch),
    }
}

fn credential_env(credential: Option<&ClaudeCredential>) -> Vec<Var> {
    match credential {
        Some(ClaudeCredential::ApiKey { key }) => Vec::from([
            Var::set("ANTHROPIC_BASE_URL", "https://api.anthropic.com"),
            Var::set("ANTHROPIC_API_KEY", key),
        ]),
        Some(ClaudeCredential::SetupToken { token }) => Vec::from([
            Var::set("ANTHROPIC_BASE_URL", "https://api.anthropic.com"),
            Var::set("CLAUDE_CODE_OAUTH_TOKEN", token),
        ]),
        Some(ClaudeCredential::Gateway { base_url, token }) => Vec::from([
            Var::set("ANTHROPIC_BASE_URL", base_url),
            Var::set("ANTHROPIC_AUTH_TOKEN", token),
        ]),
        None => Vec::new(),
    }
}

fn sandboxed_subprocess_scrub(
    credential: Option<&ClaudeCredential>,
    sandbox: Sandbox,
) -> Vec<Var> {
    match (credential, sandbox) {
        (Some(_), Sandbox::Available) => {
            Vec::from([Var::set("CLAUDE_CODE_SUBPROCESS_ENV_SCRUB", "1")])
        }
        (Some(_), Sandbox::Missing) | (None, _) => Vec::new(),
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

fn resume_args(resume: &Resume, id: &SessionId) -> Vec<String> {
    match resume {
        Resume::Fresh => strings(&["--session-id", &id.0]),
        Resume::Session(session) => {
            Vec::from([format!("--resume={}", session.0)])
        }
    }
}

#[must_use]
pub fn reported_refusal(events: Vec<ClaudeEvent>) -> Option<String> {
    events.into_iter().find_map(|event| match event {
        ClaudeEvent::TurnFinished { errors, .. } => {
            errors.first().map(|_| errors.join("; "))
        }
        _ => None,
    })
}

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
            provider: crate::PROVIDER,
            found,
            supported,
        })),
        (Err(UnsupportedVersion::Unreported), VersionPolicy::AllowUntested) => {
            Ok(Some(Note::Unversioned {
                provider: crate::PROVIDER,
            }))
        }
        (Err(unsupported), VersionPolicy::SupportedOnly) => Err(unsupported),
    }
}

#[must_use]
pub const fn unprotected(
    credential: Option<&ClaudeCredential>,
    sandbox: Sandbox,
) -> Option<Note> {
    match (credential, sandbox) {
        (Some(_), Sandbox::Missing) => Some(Note::CredentialExposed {
            provider: crate::PROVIDER,
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use agents::Effort;
    use agents::{HomeKind, Invocation, Note, Var};
    use serde_json::json;

    use super::{
        Launch, Resume, Sandbox, SettingSource, VersionPolicy, admit, cli,
        profile, reported_refusal, sandbox, session, unprotected,
    };
    use crate::{
        ClaudeCredential, ClaudeEvent, PermissionMode, SessionId,
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
                crate::PROVIDER,
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
    fn a_direct_key_uses_the_anthropic_endpoint_and_scrubs_subprocesses() {
        let launch = Launch {
            home: HomeKind::Private,
            credential: Some(ClaudeCredential::ApiKey {
                key: String::from("synthetic-api-key"),
            }),
            sandbox: Sandbox::Available,
            ..launch(Resume::Fresh)
        };
        let invoked = session(&launch);
        assert!(
            invoked
                .env
                .contains(&set("ANTHROPIC_API_KEY", "synthetic-api-key"))
        );
        assert!(
            invoked.env.contains(&set(
                "ANTHROPIC_BASE_URL",
                "https://api.anthropic.com"
            ))
        );
        assert!(
            invoked
                .env
                .contains(&set("CLAUDE_CODE_SUBPROCESS_ENV_SCRUB", "1"))
        );
        assert!(
            invoked
                .args
                .iter()
                .all(|arg| !arg.contains("synthetic-api-key"))
        );
    }

    #[test]
    fn a_setup_token_is_supplied_only_in_the_environment() {
        let launch = Launch {
            home: HomeKind::Private,
            credential: Some(ClaudeCredential::SetupToken {
                token: String::from("synthetic-setup-token"),
            }),
            sandbox: Sandbox::Available,
            ..launch(Resume::Fresh)
        };
        let invocation = session(&launch);
        assert!(invocation.env.contains(&set(
            "CLAUDE_CODE_OAUTH_TOKEN",
            "synthetic-setup-token"
        )));
        assert!(invocation.env.contains(&Var::remove("ANTHROPIC_API_KEY")));
        assert!(
            invocation
                .env
                .contains(&set("CLAUDE_CODE_SUBPROCESS_ENV_SCRUB", "1"))
        );
        assert!(
            invocation
                .args
                .iter()
                .all(|arg| !arg.contains("synthetic-setup-token"))
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
            outcome: agents::TurnOutcome::Failed {
                message: String::from("failed"),
            },
            terminal_reason: None,
            session_id: id("s"),
            errors: errors.iter().copied().map(String::from).collect(),
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
                    provider: crate::PROVIDER,
                    found: String::from("1.0.0"),
                    supported: String::from("2.1.316 to 2.1.322"),
                })),
                Ok(Some(Note::Unversioned {
                    provider: crate::PROVIDER,
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
                Some(Note::CredentialExposed {
                    provider: crate::PROVIDER
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
