//! The answers a person is putting together for an open question card,
//! before they send them. No UI types, so the rules are unit-testable.

use proto::{Answer, Question, Selection};

/// For each question, the options chosen and the text typed so far.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Draft {
    chosen: Vec<Vec<String>>,
    typed: Vec<String>,
}

impl Draft {
    pub fn new(questions: &[Question]) -> Draft {
        Draft {
            chosen: questions.iter().map(|_| Vec::new()).collect(),
            typed: questions.iter().map(|_| String::new()).collect(),
        }
    }

    /// The labels chosen for `question`.
    pub fn chosen(&self, question: usize) -> Vec<String> {
        self.chosen.get(question).cloned().unwrap_or_default()
    }

    pub fn typed(&self, question: usize) -> String {
        self.typed.get(question).cloned().unwrap_or_default()
    }

    /// Taps `label`. One choice replaces the last and clears the typed
    /// text; with many, a tap toggles the option.
    pub fn pick(self, question: usize, label: String, selection: Selection) -> Draft {
        let Draft {
            mut chosen,
            mut typed,
        } = self;
        if let Some(chosen) = chosen.get_mut(question) {
            match (selection, chosen.iter().position(|seen| seen == &label)) {
                (Selection::One, _) => {
                    chosen.clear();
                    chosen.push(label);
                }
                (Selection::Many, Some(at)) => {
                    chosen.remove(at);
                }
                (Selection::Many, None) => chosen.push(label),
            }
        }
        if let (Selection::One, Some(typed)) = (selection, typed.get_mut(question)) {
            typed.clear();
        }
        Draft { chosen, typed }
    }

    /// Types `text` as the answer. With one choice it replaces the option
    /// chosen; with many it is one more answer.
    pub fn r#type(self, question: usize, text: &str, selection: Selection) -> Draft {
        let Draft {
            mut chosen,
            mut typed,
        } = self;
        if let Some(typed) = typed.get_mut(question) {
            typed.clear();
            typed.push_str(text);
        }
        if let (Selection::One, Some(chosen)) = (selection, chosen.get_mut(question)) {
            chosen.clear();
        }
        Draft { chosen, typed }
    }

    /// Every question's answer, once every question has one.
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
                                .filter(|typed| !typed.is_empty())
                                .map(String::from),
                        )
                        .collect(),
                )
            })
            .collect::<Vec<Answer>>();
        Some(answers).filter(|answers| answers.iter().all(|answer| !answer.0.is_empty()))
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
        let draft = Draft::new(&[question(Selection::One), question(Selection::Many)]);
        let empty = draft.answers();
        let draft = draft.pick(0, String::from("Red"), Selection::One);
        let one = draft.answers();
        let draft = draft
            .pick(0, String::from("Blue"), Selection::One)
            .pick(1, String::from("Apple"), Selection::Many)
            .pick(1, String::from("Pear"), Selection::Many)
            .pick(1, String::from("Apple"), Selection::Many)
            .r#type(1, " Plum ", Selection::Many);
        let both = draft.answers();
        let draft = draft.r#type(0, "Green", Selection::One);
        let typed = draft.answers();
        let draft = draft.pick(0, String::from("Red"), Selection::One);
        assert_eq!(
            (empty, one, both, typed, draft.answers(), draft.typed(0)),
            (
                None,
                None,
                Some(Vec::from([answer(&["Blue"]), answer(&["Pear", "Plum"])])),
                Some(Vec::from([answer(&["Green"]), answer(&["Pear", "Plum"])])),
                Some(Vec::from([answer(&["Red"]), answer(&["Pear", "Plum"])])),
                String::new(),
            )
        );
    }
}
