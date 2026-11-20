use std::future::{Future, pending};

use agents::runtime::{Command, Report};
use install::runner::{self, End};
use tokio::sync::mpsc;

use crate::{Runtime, log, pair};

pub enum Performed<I> {
    Feedback(I),
    Report(Report),
    Done,
}

pub trait Agent<I, E> {
    type Event: Send;

    fn events(&mut self) -> Option<&mut mpsc::Receiver<Self::Event>>;
    fn release(&mut self);
    fn perform(
        &mut self,
        effect: E,
    ) -> impl Future<Output = Performed<I>> + Send;
}

pub fn start<I, E, A>(
    agent: A,
    step: impl FnMut(I) -> Vec<E> + Send + 'static,
    command: fn(Command) -> I,
    event: fn(Option<A::Event>) -> I,
) -> Runtime
where
    I: Send + 'static,
    E: Send + 'static,
    A: Agent<I, E> + Send + 'static,
{
    let (runtime, plug) = pair();
    tokio::spawn(runner::run(
        Task {
            agent,
            plug,
            command,
            event,
        },
        step,
    ));
    runtime
}

struct Task<I, V, A> {
    agent: A,
    plug: crate::Plug,
    command: fn(Command) -> I,
    event: fn(Option<V>) -> I,
}

impl<I, E, A> runner::Runner<I, E> for Task<I, A::Event, A>
where
    I: Send,
    E: Send,
    A: Agent<I, E> + Send,
{
    async fn next(&mut self) -> Option<I> {
        tokio::select! {
            command = self.plug.commands.recv() => command.map(self.command),
            event = next_event(self.agent.events()) => Some((self.event)(event)),
        }
    }

    async fn perform(&mut self, effect: E) -> Result<Option<I>, End> {
        match self.agent.perform(effect).await {
            Performed::Feedback(input) => return Ok(Some(input)),
            Performed::Done => {}
            Performed::Report(Report::Release) => self.agent.release(),
            Performed::Report(Report::Emit(event)) => {
                let _ = self.plug.events.send(event).await;
            }
            Performed::Report(Report::Log(note)) => log(&note),
        }
        Ok(None)
    }
}

async fn next_event<V>(events: Option<&mut mpsc::Receiver<V>>) -> Option<V> {
    match events {
        Some(receiver) => receiver.recv().await,
        None => pending().await,
    }
}
