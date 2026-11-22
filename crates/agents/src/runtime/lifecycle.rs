use proto::CloseReason;

use super::{Command, Event, Outcome, Started};
use crate::prelude::*;
use crate::{Note, Step};

pub const NOT_RUNNING: &str = "the agent is not running";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Report {
    Release,
    Emit(Event),
    Log(Note),
}

impl Report {
    #[must_use]
    pub fn emit<E: From<Self>>(outcome: Outcome) -> Vec<E> {
        Vec::from([Self::Emit(Event::Outcome(outcome)).into()])
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Lifecycle {
    running: bool,
}

impl Lifecycle {
    #[must_use]
    pub const fn new() -> Self {
        Self { running: false }
    }

    #[must_use]
    pub const fn running(&self) -> bool {
        self.running
    }

    #[must_use]
    pub fn refusal<E: From<Report>>(
        &self,
        command: &Command,
    ) -> Option<Vec<E>> {
        if self.running {
            return None;
        }
        match command {
            Command::Send { .. }
            | Command::Interrupt(_)
            | Command::Reply { .. } => {
                Some(Report::emit(command.failed(NOT_RUNNING)))
            }
            Command::Start(_) | Command::Shutdown => None,
        }
    }

    pub fn started<E: From<Report>>(&mut self, started: Started) -> Vec<E> {
        self.running = true;
        Report::emit(Outcome::Started(Ok(started)))
    }

    pub fn shut_down<E: From<Report>>(
        &mut self,
        outcome: Result<(), String>,
    ) -> Vec<E> {
        self.running = false;
        Report::emit(Outcome::ShutDown(outcome))
    }

    pub const fn release(&mut self) -> Report {
        self.running = false;
        Report::Release
    }

    pub fn ended<E: From<Report>>(&mut self) -> Vec<E> {
        if self.running {
            self.steps(
                Vec::from([Step::Closed(CloseReason::Exited {
                    status: String::from("the agent's events ended"),
                })]),
                None,
            )
        } else {
            Vec::new()
        }
    }

    pub fn steps<E: From<Report>>(
        &mut self,
        steps: Vec<Step>,
        note: Option<Note>,
    ) -> Vec<E> {
        let mut reports: Vec<E> = note
            .into_iter()
            .map(|entry| Report::Log(entry).into())
            .collect();
        for step in steps {
            let closed = matches!(step, Step::Closed(_));
            reports.push(Report::Emit(Event::Step(step)).into());
            if closed {
                reports.push(self.release().into());
            }
        }
        reports
    }
}

#[cfg(test)]
mod tests {
    use super::{Lifecycle, Report};
    use crate::runtime::{Command, Event, Outcome, Started};
    use crate::{NativeTurn, Note, Step};
    use proto::{CloseReason, Provider};

    fn start(lifecycle: &mut Lifecycle) {
        let started = Started {
            conversation: String::from("conversation"),
        };
        assert_eq!(
            lifecycle.started::<Report>(started.clone()),
            [Report::Emit(Event::Outcome(Outcome::Started(Ok(started))))],
        );
    }

    #[test]
    fn calls_require_a_running_agent_and_shutdown_clears_it() {
        let mut lifecycle = Lifecycle::new();
        let call = Command::Interrupt(NativeTurn(String::from("turn")));
        let refused = Some(vec![Report::Emit(Event::Outcome(
            Outcome::Interrupted(Err(String::from("the agent is not running"))),
        ))]);
        assert_eq!(lifecycle.refusal::<Report>(&call), refused);
        assert_eq!(lifecycle.refusal::<Report>(&Command::Shutdown), None);

        start(&mut lifecycle);
        assert_eq!(lifecycle.refusal::<Report>(&call), None);
        assert_eq!(
            lifecycle.shut_down::<Report>(Ok(())),
            [Report::Emit(Event::Outcome(Outcome::ShutDown(Ok(()))))],
        );
        assert_eq!(lifecycle.refusal::<Report>(&call), refused);
        assert_eq!(lifecycle.ended::<Report>(), Vec::<Report>::new());
    }

    #[test]
    fn closing_reports_the_reason_then_releases_once_the_events_end() {
        let mut lifecycle = Lifecycle::new();
        let frame = String::from("unmapped frame");
        assert_eq!(
            lifecycle.steps::<Report>(
                Vec::new(),
                Some(Note::unmapped(Provider::Claude, frame.clone(), None))
            ),
            [Report::Log(Note::Unmapped {
                provider: Provider::Claude,
                frame
            })],
        );
        assert_eq!(lifecycle.ended::<Report>(), Vec::<Report>::new());

        start(&mut lifecycle);
        let closed = |status: &str| {
            Step::Closed(CloseReason::Exited {
                status: String::from(status),
            })
        };
        let exited = closed("exit status: 1");
        assert_eq!(
            lifecycle.steps::<Report>(vec![exited.clone()], None),
            [Report::Emit(Event::Step(exited)), Report::Release],
        );
        assert_eq!(lifecycle.ended::<Report>(), Vec::<Report>::new());

        start(&mut lifecycle);
        assert_eq!(
            lifecycle.ended::<Report>(),
            [
                Report::Emit(Event::Step(closed("the agent's events ended"))),
                Report::Release,
            ],
        );
        assert_eq!(lifecycle.ended::<Report>(), Vec::<Report>::new());
    }
}
