//! The data exchanged between a line-JSON process runner and a driver.

use alloc::string::{String, ToString};
use core::fmt;
use serde_json::Value;

use crate::{Exit, Note};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command<C, S> {
    Call(C),
    Shutdown(S),
}

#[derive(Debug, PartialEq, Eq)]
pub enum Input<C, T, S, A, E> {
    Read(Result<Option<String>, String>),
    Command(Command<C, S>),
    Closed,
    Wrote {
        then: T,
        outcome: Result<(), E>,
    },
    Stopped {
        exit: Exit,
        stderr: Option<String>,
        then: After<S, A>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum After<S, A> {
    Nothing,
    Shutdown(S),
    Protocol(A),
    End,
}

type StopEffect<V, R, S, T, A, F> = Result<Option<Effect<V, R, S, T, A, F>>, A>;

impl<S, A> After<S, A> {
    /// # Errors
    /// Returns the protocol action that the driver must complete.
    pub fn stop<V, R, T, F>(
        self,
        exited: Option<&Exit>,
    ) -> StopEffect<V, R, S, T, A, F> {
        match exited {
            None => Ok(Some(Effect::Stop(self))),
            Some(exit) => self.complete(exit),
        }
    }

    /// # Errors
    /// Returns the protocol action that the driver must complete.
    pub fn complete<V, R, T, F>(
        self,
        exit: &Exit,
    ) -> StopEffect<V, R, S, T, A, F> {
        match self {
            Self::Nothing => Ok(None),
            Self::Shutdown(waiter) => {
                Ok(Some(Effect::Report(waiter, exit.clone())))
            }
            Self::Protocol(action) => Err(action),
            Self::End => Ok(Some(Effect::End)),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Effect<V, R, S, T, A, F> {
    /// A serialized frame. The runner adds its line terminator.
    Write {
        line: String,
        then: T,
    },
    Emit(V),
    Log(Note),
    Reply(R, Result<Value, F>),
    Report(S, Exit),
    Stop(After<S, A>),
    End,
}

#[derive(Debug)]
pub enum Failure<E, P> {
    Exited,
    Refused(String),
    Io(E),
    Json(serde_json::Error),
    Protocol(P),
}

impl<E: PartialEq, P: PartialEq> PartialEq for Failure<E, P> {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Exited, Self::Exited) => true,
            (Self::Refused(left), Self::Refused(right)) => left == right,
            (Self::Io(left), Self::Io(right)) => left == right,
            (Self::Json(left), Self::Json(right)) => {
                left.to_string() == right.to_string()
            }
            (Self::Protocol(left), Self::Protocol(right)) => left == right,
            _ => false,
        }
    }
}

pub struct FailureText {
    pub exited: &'static str,
    pub io: &'static str,
    pub json: &'static str,
}

impl<E: fmt::Display, P: fmt::Display> Failure<E, P> {
    #[must_use]
    pub fn describe(&self, text: &FailureText) -> String {
        match self {
            Self::Exited => String::from(text.exited),
            Self::Refused(reason) => {
                alloc::format!("{}: {reason}", text.exited)
            }
            Self::Io(error) => alloc::format!("{}: {error}", text.io),
            Self::Json(error) => alloc::format!("{}: {error}", text.json),
            Self::Protocol(error) => error.to_string(),
        }
    }
}
