//! Runs an effect and its feedback before the next effect in the batch.

use std::future::Future;

pub trait Runner<I, E> {
    fn next(&mut self) -> impl Future<Output = Option<I>> + Send;
    fn perform(
        &mut self,
        effect: E,
    ) -> impl Future<Output = Result<Option<I>, End>> + Send;
}

pub struct End;

pub async fn run<I, E>(
    mut runner: impl Runner<I, E>,
    mut step: impl FnMut(I) -> Vec<E>,
) {
    while let Some(input) = runner.next().await {
        if feed(&mut runner, &mut step, input).await.is_err() {
            return;
        }
    }
}

async fn feed<I, E>(
    runner: &mut impl Runner<I, E>,
    step: &mut impl FnMut(I) -> Vec<E>,
    input: I,
) -> Result<(), End> {
    let mut effects = step(input);
    effects.reverse();
    while let Some(effect) = effects.pop() {
        if let Some(feedback) = runner.perform(effect).await? {
            effects.extend(step(feedback).into_iter().rev());
        }
    }
    Ok(())
}
