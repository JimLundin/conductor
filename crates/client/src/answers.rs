//! The answers a person is putting together for an open question card,
//! before they send them. No UI types, so the rules are unit-testable.

use crate::prelude::*;
use proto::{Answer, Question, Selection};

/// For each question, the options chosen and the text typed so far.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Draft {
    chosen: Vec<Vec<String>>,
    typed: Vec<String>,
}

impl Draft {
    #[must_use]
    pub fn new(questions: &[Question]) -> Self {
        Self {
            chosen: questions.iter().map(|_| Vec::new()).collect(),
            typed: questions.iter().map(|_| String::new()).collect(),
        }
    }

    /// The labels chosen for `question`.
    #[must_use]
    pub fn chosen(&self, question: usize) -> Vec<String> {
        self.chosen.get(question).cloned().unwrap_or_default()
    }

    #[must_use]
    pub fn typed(&self, question: usize) -> String {
        self.typed.get(question).cloned().unwrap_or_default()
    }

    /// Taps `label`. One choice replaces the last and clears the typed
    /// text; with many, a tap toggles the option.
    #[must_use]
    pub fn pick(
        self,
        question: usize,
        label: String,
        selection: Selection,
    ) -> Self {
        let Self {
            mut chosen,
            mut typed,
        } = self;
        if let Some(labels) = chosen.get_mut(question) {
            match (selection, labels.iter().position(|seen| seen == &label)) {
                (Selection::One, _) => {
                    labels.clear();
                    labels.push(label);
                }
                (Selection::Many, Some(at)) => {
                    labels.remove(at);
                }
                (Selection::Many, None) => labels.push(label),
            }
        }
        if let (Selection::One, Some(text)) =
            (selection, typed.get_mut(question))
        {
            text.clear();
        }
        Self { chosen, typed }
    }

    /// Types `text` as the answer. With one choice it replaces the option
    /// chosen; with many it is one more answer.
    #[must_use]
    pub fn r#type(
        self,
        question: usize,
        text: &str,
        selection: Selection,
    ) -> Self {
        let Self {
            mut chosen,
            mut typed,
        } = self;
        if let Some(answer) = typed.get_mut(question) {
            answer.clear();
            answer.push_str(text);
        }
        if let (Selection::One, Some(labels)) =
            (selection, chosen.get_mut(question))
        {
            labels.clear();
        }
        Self { chosen, typed }
    }

    /// Every question's answer, once every question has one.
    #[must_use]
    pub fn answers(&self) -> Option<Vec<Answer>> {
        let answers = self
            .chosen
            .iter()
            .zip(&self.typed)
            .map(|(chosen, typed)| {
                Answer(
                    chosen
                        .iter()
                        .cloned()
                        .chain(
                            Some(typed.trim())
                                .filter(|trimmed| !trimmed.is_empty())
                                .map(String::from),
                        )
                        .collect(),
                )
            })
            .collect::<Vec<Answer>>();
        Some(answers)
            .filter(|all| all.iter().all(|answer| !answer.0.is_empty()))
    }
}

#[cfg(test)]
mod tests {
    use proto::{Answer, Question, Selection};

    use super::Draft;

    fn question(selection: Selection) -> Question {
        Question {
            header: String::from("Q"),
            text: String::from("Which?"),
            options: Vec::new(),
            selection,
        }
    }

    fn answer(words: &[&str]) -> Answer {
        Answer(words.iter().copied().map(String::from).collect())
    }

    #[test]
    fn every_question_needs_an_answer_before_the_draft_is_sent() {
        let fresh =
            Draft::new(&[question(Selection::One), question(Selection::Many)]);
        let empty = fresh.answers();
        let first_only = fresh.pick(0, String::from("Red"), Selection::One);
        let one = first_only.answers();
        let complete = first_only
            .pick(0, String::from("Blue"), Selection::One)
            .pick(1, String::from("Apple"), Selection::Many)
            .pick(1, String::from("Pear"), Selection::Many)
            .pick(1, String::from("Apple"), Selection::Many)
            .r#type(1, " Plum ", Selection::Many);
        let both = complete.answers();
        let retyped = complete.r#type(0, "Green", Selection::One);
        let typed = retyped.answers();
        let repicked = retyped.pick(0, String::from("Red"), Selection::One);
        assert_eq!(
            (
                empty,
                one,
                both,
                typed,
                repicked.answers(),
                repicked.typed(0)
            ),
            (
                None,
                None,
                Some(Vec::from([answer(&["Blue"]), answer(&["Pear", "Plum"])])),
                Some(Vec::from([
                    answer(&["Green"]),
                    answer(&["Pear", "Plum"])
                ])),
                Some(Vec::from([answer(&["Red"]), answer(&["Pear", "Plum"])])),
                String::new(),
            )
        );
    }
    #[test]
    fn the_typed_text_is_kept_per_question() {
        let draft =
            Draft::new(&[question(Selection::One), question(Selection::One)])
                .r#type(1, "Green", Selection::One);
        assert_eq!(
            [draft.typed(0), draft.typed(1), draft.typed(2)],
            [String::new(), String::from("Green"), String::new()]
        );
    }
}
