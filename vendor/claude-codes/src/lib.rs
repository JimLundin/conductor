//! Typed bindings for the Claude Code CLI's stream-json protocol.
//!
//! - [`io`] holds the frames: [`ClaudeInput`] for what we write to stdin and
//!   [`ClaudeOutput`] for each stdout line.
//! - [`cli`] builds the `claude` command line ([`ClaudeCliBuilder`]).
//!
//! The crate does not own a process. The caller spawns the command, owns its
//! stdio, and parses each line with these types.
//!
//! The `types` feature has no native dependencies and compiles to WASM. The
//! `cli` feature adds the command builder and pulls in tokio.

// Core modules always available
pub mod error;
pub mod io;
pub mod messages;
pub mod models;
pub mod protocol;
pub mod tool_inputs;
pub mod types;

// On-disk transcript locations (the CLI's unpublished path-encoding rule)
pub mod transcript;

// Login support tooling (PTY-driven `claude auth login` / `setup-token`)
#[cfg(feature = "auth")]
pub mod auth;

#[cfg(feature = "cli")]
pub mod cli;
#[cfg(feature = "cli")]
mod process;

// Core exports always available
pub use error::{Error, Result};
pub use io::{
    AnthropicError, AnthropicErrorDetails, ApiErrorType, AssistantMessageContent, ClaudeInput,
    ClaudeOutput, ParseError, TranscriptMessage,
};
pub use messages::*;
pub use models::{ClaudeModel, EffortLevel};
pub use protocol::{MessageEnvelope, Protocol};
pub use types::*;

// Content block types for message parsing
pub use io::{
    CodeExecutionToolResultBlock, ContainerUploadBlock, ContentBlock, FallbackBlock, FallbackModel,
    ImageBlock, ImageSource, ImageSourceType, McpToolResultBlock, McpToolUseBlock, MediaType,
    ServerToolUseBlock, TextBlock, ThinkingBlock, ToolResultBlock, ToolResultContent,
    WebSearchToolResultBlock,
};

// Control protocol types for tool permission handling
pub use io::{
    ApplyFlagSettingsRequest, AskUserQuestionResponseError, ControlCancelRequest, ControlRequest,
    ControlRequestMessage, ControlRequestPayload, ControlResponse, ControlResponseMessage,
    ControlResponsePayload, GetUsageResponse, HookCallbackRequest, InitializeRequest,
    McpMessageRequest, ModelScopedRateLimit, Permission, PermissionBehavior, PermissionDenial,
    PermissionDestination, PermissionMode, PermissionModeName, PermissionResult, PermissionRule,
    PermissionSuggestion, PermissionType, ToolCaller, ToolPermissionRequest, ToolUseBlock,
    UsageBehavior, UsageBehaviors, UsageModelUsage, UsageRateLimitWindow, UsageRateLimits,
    UsageSession,
};

// System message and assistant message types
pub use io::{
    ApiErrorCut, ApiErrorMedia, ApiErrorMediaReason, ApiErrorParams, ApiErrorProvider,
    ApiErrorRemedy, ApiKeySource, ApiRetryMessage, ApiRetryNoResponse, ArtifactReadVersion,
    AssistantErrorKind, AutoModeRule, AutoModeRuleList, BackgroundTaskInfo,
    BackgroundTasksChangedMessage, BatchToolUse, ClientThinking, CloudSessionDeltaMessage,
    CodeChangePublishedMessage, CommandInfo, CommandsChangedMessage, CompactBoundaryMessage,
    CompactMetadata, CompactionTrigger, ContextAgent, ContextCategory, ContextMcpTool,
    ContextMemoryFile, ContextOverLimit, ContextSkill, ContextUsage, ControlRequestProgressMessage,
    CyberVerificationStatus, DevIntentKind, DevIntentMessage, DevIntentTrigger,
    ElicitationCompleteMessage, FailedPersistedFile, FeedbackDraftQueuedMessage,
    FileAttachmentsMissingMessage, FilesPersistedMessage, HandbackReport, HookProgressMessage,
    HookResponseMessage, HookStartedMessage, InformationalMessage, InheritedAsksSettledMessage,
    InitMessage, InitPermissionMode, InstructionSizeWarningMessage, KnownSystemEvent,
    LocalCommandOutcome, LocalCommandOutcomeKind, LocalCommandOutputMessage, LocalCommandRun,
    LostSessionWrite, McpMeta, McpServerError, MemoryPaths, MemoryRecallItem, MemoryRecallMessage,
    MessageOrigin, MessageRole, MirrorErrorKey, MirrorErrorMessage, MissingFileAttachment,
    ModelRefusalFallbackMessage, ModelRefusalNoFallbackMessage, ModelRestriction,
    NotificationMessage, OutputStyle, PeerMessageHoldCause, PeerMessageHoldMessage,
    PeerMessageHoldOutcome, PeerMessageHoldState, PeerMessageLane, PerTurnEffortChangedMessage,
    PermissionCheckStatus, PermissionCheckStatusMessage, PermissionDecisionOutcome,
    PermissionDecisionReasonType, PermissionDeniedMessage, PermissionDeniedReasonCode,
    PersistedFile, PluginDiagnostic, PluginInfo, PluginInstallMessage, PluginPrompt,
    PreservedMessages, PreservedSegment, PromptSubmitOutcome, PromptSubmitOutcomeKind,
    RefusalFallbackScope, RemedyFeature, RemedyFeatureCause, RemedyLoginProvider, RemedyPolicyKind,
    SessionTitleChangedMessage, SessionWriteErrorCode, SessionWritesLostMessage, StatusMessage,
    StatusMessageStatus, StopReason, SummarizeMetadata, SystemMessage, SystemSubtype,
    TaskEndReason, TaskHandback, TaskNotificationMessage, TaskPatch, TaskProgressMessage,
    TaskStartedMessage, TaskStatus, TaskType, TaskUpdatedMessage, TaskUsage, ThinkingTokensMessage,
    ToolPermissionDecision, ToolResultMeta, ToolResultRemedy, ToolResultRemedyKind, ToolUseMeta,
    TurnHandoffAvailableMessage, TurnPreemptedMessage, UiFocusMessage, UiInvalidateMessage,
    UiInvalidatedInstance, UiLogMessage, UiNotifyCancelMessage, UiNotifyMessage, UiPane,
    UiPanesMessage, UiPlacedMessage, UiPlacedRow, UiScrollMessage, UiSiteComponent,
    UiStatusMessage, UiToastMessage, UsageReport, UsageReportExtraUsage, UsageReportLimit,
    UsageReportRateLimits, UsageReportScope, UsageReportScopeLabel, VcsMutationKind,
    VcsStateChangedMessage, ViewMode, WorkerShuttingDownMessage,
};

// Additional top-level output message wrappers
pub use io::{
    AbandonedBlocks, AuthStatusMessage, CommandLifecycleMessage, CommandLifecycleState,
    CommandWakeupSource, ConversationResetMessage, ConversationResetTrigger,
    PromptSuggestionMessage, StreamEventMessage, SubagentRetry, ToolProgressMessage,
    ToolUseSummaryMessage,
};

// Wire-fidelity audit for verifying frames are fully typed
pub use io::{assert_fully_wrapped, audit_frame, FrameAudit};

// Rate limit types
pub use io::{
    AdminRequestOffer, AdminRequestOfferState, OverageDisabledReason, OveragePeriodUtilization,
    OverageStatus, RateLimitErrorCode, RateLimitEvent, RateLimitInfo, RateLimitScope,
    RateLimitStatus, RateLimitWindow, UsageCreditOffer, UsageCreditOfferKind,
};

// Usage types
pub use io::{
    AssistantUsage, CacheCreationDetails, DeferredToolUse, DiffersFilesCounts,
    FastModeDisabledReason, GarbageCollectionCounts, InputAttachmentsDetail,
    MessageNormalizationDetail, ProcessGarbageCollections, ProcessStateTime, ProcessTurnStates,
    RequestBuildDetail, ResumeStoreConfirmDetail, RunnerExit, RunnerExitPhase, ServerToolUse,
    StartupFailureReason, StreamPostQueuedBehind, SubagentKillCounts, SubagentRefusalCounts,
    SubagentResult, SubagentSpawnRequests, SubagentStats, SubagentToolStats, SubagentUsageRollup,
    ToolSchemaBuildDetail, TouchCompareCounts, TurnStartResumeKind, UsageInfo,
};

// Typed tool input types
pub use tool_inputs::{
    AllowedPrompt, AskUserQuestionInput, BashInput, EditInput, EnterPlanModeInput,
    ExitPlanModeInput, GlobInput, GrepInput, GrepOutputMode, KillShellInput, LsInput,
    MultiEditInput, MultiEditOperation, NotebookCellType, NotebookEditInput, NotebookEditMode,
    NotebookReadInput, Question, QuestionMetadata, QuestionOption, ReadInput, ScheduleWakeupInput,
    SkillInput, SubagentType, TaskInput, TaskOutputInput, TodoItem, TodoStatus, TodoWriteInput,
    ToolInput, ToolSearchInput, WebFetchInput, WebSearchInput, WriteInput,
};

#[cfg(feature = "cli")]
pub use cli::{ClaudeCliBuilder, SettingSource};
