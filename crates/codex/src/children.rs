use alloc::collections::BTreeSet;

use agents::{NativeRequest, Step};
use proto::{
    EventKind, SubagentId, SubagentOutcome, ToolCallId, ToolKind, ToolOutcome,
    ToolPhase, TurnOutcome,
};

use crate::mapping::codex_thread_steps;
use crate::prelude::*;
use crate::{
    ApprovalId, ChildActivity, ChildEvent, ChildState, ChildStatus,
    ThreadEvent, ThreadId,
};

#[derive(Debug, PartialEq, Eq, Default)]
pub struct Children {
    agents: BTreeMap<ThreadId, Child>,
    calls: BTreeSet<(ThreadId, String)>,
}

#[derive(Debug, PartialEq, Eq)]
struct Child {
    parent: ThreadId,
    root: ThreadId,
    call: ToolCallId,
    description: String,
    kind: Option<String>,
    ended: Option<SubagentOutcome>,
    summary: Option<String>,
}

impl Children {
    pub const fn new() -> Self {
        Self {
            agents: BTreeMap::new(),
            calls: BTreeSet::new(),
        }
    }

    pub fn root(&self, child: &ThreadId) -> Option<&ThreadId> {
        self.agents.get(child).map(|agent| &agent.root)
    }

    pub fn forget(&mut self, root: &ThreadId) {
        self.agents.retain(|_, agent| &agent.root != root);
        self.calls.retain(|(owner, _)| owner != root);
    }

    pub fn notice(
        &mut self,
        thread: &ThreadId,
        event: ChildEvent,
        has_route: impl Fn(&ThreadId) -> bool,
    ) -> Option<(ThreadId, Vec<Step>)> {
        let parent = match &event {
            ChildEvent::ThreadStarted { parent, .. } => parent.clone(),
            _ => thread.clone(),
        };
        let root = if has_route(&parent) {
            parent.clone()
        } else {
            self.root(&parent)?.clone()
        };
        let mut steps = Vec::new();
        match event {
            ChildEvent::ThreadStarted { name, kind, .. }
                if !has_route(thread) =>
            {
                let call = ToolCallId(format!("subagent:{}", thread.0));
                let description =
                    name.unwrap_or_else(|| String::from("Subagent"));
                steps.extend(self.register(
                    thread.clone(),
                    Child::new(
                        parent.clone(),
                        root.clone(),
                        call,
                        description,
                        kind,
                    ),
                ));
            }
            ChildEvent::Collaboration { spawn, states } => {
                if let Some(started) = spawn {
                    for agent in started
                        .agents
                        .into_iter()
                        .filter(|agent| !has_route(agent))
                    {
                        let description = started
                            .description
                            .clone()
                            .unwrap_or_else(|| String::from("Subagent"));
                        steps.extend(self.register(
                            agent,
                            Child::new(
                                parent.clone(),
                                root.clone(),
                                started.call.clone(),
                                description,
                                None,
                            ),
                        ));
                    }
                }
                for (agent, state) in states {
                    if let Some(child) = self
                        .agents
                        .get_mut(&agent)
                        .filter(|child| child.parent == parent)
                    {
                        steps.extend(child.status(&agent, state));
                    }
                }
            }
            ChildEvent::Activity {
                call,
                agent,
                path,
                activity,
            } if !has_route(&agent) => {
                steps.extend(
                    self.activity(&parent, &root, call, agent, path, activity),
                );
            }
            ChildEvent::ThreadStarted { .. } | ChildEvent::Activity { .. } => {}
        }
        Some((root, steps))
    }

    fn register(&mut self, id: ThreadId, child: Child) -> Vec<Step> {
        if id == child.root {
            return Vec::new();
        }
        if let Some(existing) = self.agents.get_mut(&id) {
            if existing.parent != child.parent {
                return Vec::new();
            }
            let changed = child
                .kind
                .as_ref()
                .is_some_and(|kind| existing.kind.as_ref() != Some(kind));
            if changed {
                existing.kind = child.kind;
                return existing
                    .kind
                    .clone()
                    .map(|kind| Step::SubagentKind {
                        agent: SubagentId(id.0),
                        kind,
                    })
                    .into_iter()
                    .collect();
            }
            return Vec::new();
        }
        let mut steps = Vec::new();
        if self
            .calls
            .insert((child.root.clone(), child.call.0.clone()))
        {
            let launch = Step::Emit(EventKind::ToolCall {
                call: child.call.clone(),
                phase: ToolPhase::Started {
                    tool: ToolKind::Other {
                        name: String::from("Spawn agent"),
                        detail: child.description.clone(),
                    },
                },
            });
            let finish = Step::ToolFinished {
                call: child.call.clone(),
                outcome: ToolOutcome::Succeeded,
            };
            for step in [launch, finish] {
                if child.parent == child.root {
                    steps.push(step);
                } else {
                    steps.extend(
                        step.for_subagent(SubagentId(child.parent.0.clone())),
                    );
                }
            }
        }
        steps.push(child.started(&id));
        self.agents.insert(id, child);
        steps
    }

    fn activity(
        &mut self,
        parent: &ThreadId,
        root: &ThreadId,
        call: ToolCallId,
        agent: ThreadId,
        path: String,
        activity: ChildActivity,
    ) -> Vec<Step> {
        match activity {
            ChildActivity::Started => self.register(
                agent,
                Child::new(parent.clone(), root.clone(), call, path, None),
            ),
            ChildActivity::Interrupted | ChildActivity::Completed => self
                .agents
                .get_mut(&agent)
                .filter(|child| &child.parent == parent)
                .and_then(|child| {
                    let outcome = if activity == ChildActivity::Completed {
                        SubagentOutcome::Finished
                    } else {
                        SubagentOutcome::Stopped
                    };
                    child.end(&agent, outcome, None)
                })
                .into_iter()
                .collect(),
            ChildActivity::Interacted => Vec::new(),
        }
    }

    pub fn event(&mut self, id: &ThreadId, event: ThreadEvent) -> Vec<Step> {
        let Some(child) = self.agents.get_mut(id) else {
            return Vec::new();
        };
        match event {
            ThreadEvent::TurnStarted { .. } => {
                if child.ended.take().is_some() {
                    child.summary = None;
                    Vec::from([child.started(id)])
                } else {
                    Vec::new()
                }
            }
            ThreadEvent::TurnCompleted { outcome, .. } => {
                let (ended, summary) = match outcome {
                    TurnOutcome::Completed => (SubagentOutcome::Finished, None),
                    TurnOutcome::Interrupted => {
                        (SubagentOutcome::Stopped, None)
                    }
                    TurnOutcome::Failed { message } => {
                        (SubagentOutcome::Failed, Some(message))
                    }
                };
                child.end(id, ended, summary).into_iter().collect()
            }
            other => codex_thread_steps(other)
                .into_iter()
                .filter_map(|mut step| {
                    let request = match &mut step {
                        Step::Open { native, .. }
                        | Step::Ask { native, .. }
                        | Step::Withdraw(native) => Some(native),
                        _ => None,
                    };
                    if let Some(native) = request {
                        *native = child_request(id, &native.0);
                    }
                    step.for_subagent(SubagentId(id.0.clone()))
                })
                .collect(),
        }
    }
}

impl Child {
    const fn new(
        parent: ThreadId,
        root: ThreadId,
        call: ToolCallId,
        description: String,
        kind: Option<String>,
    ) -> Self {
        Self {
            parent,
            root,
            call,
            description,
            kind,
            ended: None,
            summary: None,
        }
    }

    fn started(&self, id: &ThreadId) -> Step {
        Step::SubagentStarted {
            agent: SubagentId(id.0.clone()),
            call: self.call.clone(),
            description: self.description.clone(),
            kind: self.kind.clone(),
        }
    }

    fn status(&mut self, id: &ThreadId, state: ChildState) -> Option<Step> {
        let outcome = match state.status {
            ChildStatus::Completed => SubagentOutcome::Finished,
            ChildStatus::Errored => SubagentOutcome::Failed,
            ChildStatus::Interrupted
            | ChildStatus::Shutdown
            | ChildStatus::NotFound => SubagentOutcome::Stopped,
            ChildStatus::PendingInit | ChildStatus::Running => return None,
        };
        self.end(
            id,
            outcome,
            state.message.filter(|message| !message.is_empty()),
        )
    }

    fn end(
        &mut self,
        id: &ThreadId,
        outcome: SubagentOutcome,
        summary: Option<String>,
    ) -> Option<Step> {
        if self.ended.is_some()
            && (summary.is_none() || summary == self.summary)
        {
            return None;
        }
        self.ended = Some(outcome);
        self.summary.clone_from(&summary);
        Some(Step::SubagentEnded {
            agent: SubagentId(id.0.clone()),
            outcome,
            summary,
        })
    }
}

fn child_request(thread: &ThreadId, id: &str) -> NativeRequest {
    NativeRequest(format!("child:{}:{}{id}", thread.0.len(), thread.0))
}

pub fn request(native: NativeRequest) -> (Option<ThreadId>, ApprovalId) {
    let child = native.0.strip_prefix("child:").and_then(|value| {
        let (size, joined) = value.split_once(':')?;
        joined.split_at_checked(size.parse::<usize>().ok()?)
    });
    match child {
        Some((thread, id)) => {
            (Some(ThreadId(thread.to_owned())), ApprovalId(id.to_owned()))
        }
        None => (None, ApprovalId(native.0)),
    }
}
