use alloc::{
    collections::BTreeSet,
    format,
    string::{String, ToString},
    vec::Vec,
};
use home::flow::command::{Change, Mutation};
use home::flow::{Flow, Role};
use home::issue::{
    ChecklistChange, Datum, IssueCreate, IssueWrite, KIND, NativeIssue, Part,
    STAGE, STATUS,
    body::{self, Body},
    document::relation_value,
};
use home::{IssueKey, ProjectId};

use crate::configuration::{Configuration, FieldSource, RelationSource};

const FOOTER: &str = "project-data";

struct Planned<'a> {
    flow: &'a Flow,
    project: ProjectId,
    config: &'a Configuration,
    observed: &'a NativeIssue,
    labels: Vec<String>,
    open: bool,
    title: Option<String>,
    human: Option<Body>,
    footer: Option<toml::Table>,
}

pub fn writes(
    flow: &Flow,
    config: &Configuration,
    project: ProjectId,
    observed: &NativeIssue,
    mutations: &[Mutation],
) -> Result<Vec<IssueWrite>, String> {
    let mut planned = Planned {
        flow,
        project,
        config,
        observed,
        labels: observed.labels.clone(),
        open: observed.open,
        title: None,
        human: None,
        footer: None,
    };
    for mutation in mutations {
        planned.apply(mutation)?;
    }
    planned.writes()
}

pub fn opening(
    flow: &Flow,
    config: &Configuration,
    change: &Change,
    operation: &str,
) -> Result<IssueCreate, String> {
    let (issue, body) = change.observation.editable()?;
    let blank = NativeIssue {
        title: String::new(),
        body: String::new(),
        labels: Vec::new(),
        open: true,
        reason: None,
    };
    let mut planned = Planned {
        flow,
        project: issue.project,
        config,
        observed: &blank,
        labels: Vec::new(),
        open: true,
        title: None,
        human: Some(Body::of(&body.body_text())),
        footer: None,
    };
    planned.label(KIND, &issue.kind)?;
    if issue.status != config.defaults.open {
        planned.label(STATUS, &issue.status)?;
    }
    if let Some(stage) = &issue.stage {
        planned.label(STAGE, stage)?;
    }
    for mutation in &change.mutations {
        match mutation {
            Mutation::Set { field, .. }
                if field == STAGE || planned.created(field) => {}
            other => planned.apply(other)?,
        }
    }
    planned.footer()?.insert(
        String::from("operation"),
        toml::Value::String(String::from(operation)),
    );
    Ok(IssueCreate {
        title: issue.title.clone(),
        body: planned.written_body()?.unwrap_or_default(),
        labels: planned.labels,
    })
}

impl Planned<'_> {
    fn created(&self, field: &str) -> bool {
        self.config.fields.iter().any(|mapping| {
            mapping.name == field && mapping.source == FieldSource::CreationTime
        })
    }

    fn footer(&mut self) -> Result<&mut toml::Table, String> {
        if self.footer.is_none() {
            let read =
                match self.parts()?.1 {
                    Some(data) => toml::from_str::<toml::Table>(data).map_err(
                        |error| format!("invalid project-data: {error}"),
                    )?,
                    None => toml::Table::from_iter([(
                        String::from("schema"),
                        toml::Value::Integer(1),
                    )]),
                };
            self.footer = Some(read);
        }
        self.footer
            .as_mut()
            .ok_or_else(|| String::from("the footer doesn't read"))
    }

    fn apply(&mut self, mutation: &Mutation) -> Result<(), String> {
        match mutation {
            Mutation::Move { part, to } => {
                let field = match part {
                    Part::Stage => STAGE,
                    Part::Status => STATUS,
                };
                self.label(field, to)?;
                if *part == Part::Status {
                    self.open = !matches!(
                        self.flow.status(to).map(|status| status.role),
                        Some(Role::Terminal | Role::Dropped)
                    );
                }
                Ok(())
            }
            Mutation::Set { field, value } => self.set(field, value),
            Mutation::Relate {
                relation,
                many,
                targets,
            } => self.relate(relation, *many, targets),
            Mutation::Title(title) => {
                self.title = Some(title.clone());
                Ok(())
            }
            Mutation::Body(text) => {
                self.body()?.body(text);
                Ok(())
            }
            Mutation::Checklist(change) => {
                let body = self.body()?;
                match change {
                    ChecklistChange::Add(text) => {
                        body.add_item(text);
                        Some(())
                    }
                    ChecklistChange::Check { item, done } => {
                        body.check(item.index, *done)
                    }
                    ChecklistChange::Remove(item) => {
                        body.remove_item(item.index)
                    }
                }
                .ok_or_else(|| String::from("its checklist doesn't read"))
            }
            Mutation::Note(_) => Ok(()),
        }
    }

    fn label(&mut self, field: &str, value: &str) -> Result<(), String> {
        let values = self
            .config
            .fields
            .iter()
            .find(|mapping| mapping.name == field)
            .and_then(|mapping| match &mapping.source {
                FieldSource::ManagedLabels { values } => Some(values),
                _ => None,
            })
            .ok_or_else(|| format!("{field} has no GitHub label mapping"))?;
        let wanted = values
            .get(value)
            .ok_or_else(|| format!("no GitHub label maps {field} {value:?}"))?;
        let others: Vec<String> = values
            .values()
            .filter(|label| *label != wanted)
            .map(|label| label.to_lowercase())
            .collect();
        self.labels
            .retain(|label| !others.contains(&label.to_lowercase()));
        if !self.has(wanted) {
            self.labels.push(wanted.clone());
        }
        Ok(())
    }

    fn has(&self, name: &str) -> bool {
        let lowercase = name.to_lowercase();
        self.labels
            .iter()
            .any(|label| label.to_lowercase() == lowercase)
    }

    fn set(&mut self, field: &str, value: &Datum) -> Result<(), String> {
        if let Some(mapping) = self
            .config
            .fields
            .iter()
            .find(|mapping| mapping.name == field)
        {
            return match (&mapping.source, value) {
                (FieldSource::ManagedLabels { .. }, Datum::Text(text)) => {
                    self.label(field, text)
                }
                (FieldSource::OrdinaryLabels, Datum::Texts(wanted)) => {
                    self.ordinary(wanted);
                    Ok(())
                }
                (FieldSource::Metadata, _) => {
                    self.footer_entry("fields", field, datum(value))
                }
                (FieldSource::CreationTime, _) => Err(format!(
                    "{field} is the issue's creation time on GitHub, which doesn't change"
                )),
                _ => Err(format!("{field} doesn't take that value on GitHub")),
            };
        }
        Err(format!("{field} has no GitHub mapping"))
    }

    fn ordinary(&mut self, wanted: &[String]) {
        let namespace = self.config.namespace.to_lowercase();
        let lowered: Vec<String> =
            wanted.iter().map(|label| label.to_lowercase()).collect();
        self.labels.retain(|label| {
            let lowercase = label.to_lowercase();
            lowercase.starts_with(&namespace) || lowered.contains(&lowercase)
        });
        for label in wanted {
            if !self.has(label) {
                self.labels.push(label.clone());
            }
        }
    }

    fn parts(&self) -> Result<(&str, Option<&str>), String> {
        let text = self.observed.body.as_str();
        Ok(match body::footer(text, FOOTER)? {
            Some((human, data)) => (human, Some(data)),
            None => (text, None),
        })
    }

    fn body(&mut self) -> Result<&mut Body, String> {
        if self.human.is_none() {
            self.human = Some(Body::of(self.parts()?.0));
        }
        self.human
            .as_mut()
            .ok_or_else(|| String::from("the body doesn't read"))
    }

    fn footer_entry(
        &mut self,
        table: &str,
        name: &str,
        value: Option<toml::Value>,
    ) -> Result<(), String> {
        let entries = self
            .footer()?
            .entry(table)
            .or_insert_with(|| toml::Value::Table(toml::Table::new()))
            .as_table_mut()
            .ok_or_else(|| format!("project-data {table} isn't a table"))?;
        match value {
            Some(kept) => {
                entries.insert(name.into(), kept);
            }
            None => {
                entries.remove(name);
            }
        }
        Ok(())
    }

    fn relate(
        &mut self,
        relation: &str,
        many: bool,
        targets: &[IssueKey],
    ) -> Result<(), String> {
        match self
            .config
            .relations
            .iter()
            .find(|mapping| mapping.name == relation)
            .map(|mapping| mapping.source)
        {
            Some(RelationSource::Metadata) => self.footer_entry(
                "relations",
                relation,
                (!targets.is_empty())
                    .then(|| relation_value(many, targets, self.project)),
            ),
            Some(RelationSource::Parent | RelationSource::Dependency) => {
                Err(format!(
                    "{relation} is a native GitHub relation, which home doesn't write yet (#140)"
                ))
            }
            None => Err(format!("{relation} has no GitHub mapping")),
        }
    }

    fn written_body(&self) -> Result<Option<String>, String> {
        if self.human.is_none() && self.footer.is_none() {
            return Ok(None);
        }
        let (read, _) = self.parts()?;
        let footer_text = self.observed.body.get(read.len()..).unwrap_or("");
        let human = self.human.as_ref().map_or_else(
            || String::from(read),
            |body| String::from(body.text().trim_start_matches('\n')),
        );
        let footer = match &self.footer {
            Some(table) => format!(
                "```{FOOTER}\n{}```\n",
                toml::to_string(table).map_err(|error| error.to_string())?
            ),
            None => String::from(footer_text),
        };
        let separated =
            if footer.is_empty() || human.is_empty() || human.ends_with("\n\n")
            {
                ""
            } else if human.ends_with('\n') {
                "\n"
            } else {
                "\n\n"
            };
        Ok(Some(format!("{human}{separated}{footer}")))
    }

    fn writes(&self) -> Result<Vec<IssueWrite>, String> {
        let observed = self.observed;
        let mut writes: Vec<IssueWrite> = self
            .labels
            .iter()
            .filter(|label| !observed.labelled(label))
            .cloned()
            .map(IssueWrite::AddLabel)
            .collect();
        let kept: BTreeSet<String> = self
            .labels
            .iter()
            .map(|label| label.to_lowercase())
            .collect();
        writes.extend(
            observed
                .labels
                .iter()
                .filter(|label| !kept.contains(&label.to_lowercase()))
                .cloned()
                .map(IssueWrite::RemoveLabel),
        );
        match (observed.open, self.open) {
            (true, false) => writes.push(IssueWrite::Close {
                reason: String::from(self.reason()),
                was: observed.reason.clone(),
            }),
            (false, true) => writes.push(IssueWrite::Reopen {
                was: observed.reason.clone(),
            }),
            _ => {}
        }
        if let Some(title) = &self.title
            && *title != observed.title
        {
            writes.push(IssueWrite::Title {
                from: observed.title.clone(),
                to: title.clone(),
            });
        }
        if let Some(body) = self.written_body()?
            && body != observed.body
        {
            writes.push(IssueWrite::Body {
                from: observed.body.clone(),
                to: body,
            });
        }
        Ok(writes)
    }

    fn reason(&self) -> &'static str {
        let dropped = self.config.fields.iter().any(|mapping| {
            mapping.name == STATUS
                && matches!(&mapping.source, FieldSource::ManagedLabels { values }
                    if values.iter().any(|(status, label)| {
                        self.has(label)
                            && self.flow.status(status).map(|declared| declared.role)
                                == Some(Role::Dropped)
                    }))
        });
        if dropped { "not_planned" } else { "completed" }
    }
}

fn datum(value: &Datum) -> Option<toml::Value> {
    match value {
        Datum::Text(text) if text.is_empty() => None,
        Datum::Texts(texts) if texts.is_empty() => None,
        Datum::Text(text) => Some(toml::Value::String(text.clone())),
        Datum::Texts(texts) => Some(toml::Value::Array(
            texts.iter().cloned().map(toml::Value::String).collect(),
        )),
        Datum::Bool(flag) => Some(toml::Value::Boolean(*flag)),
    }
}
