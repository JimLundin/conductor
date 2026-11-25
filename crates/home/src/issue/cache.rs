use super::{
    Freshness, IssueListing, IssueReading, IssueState, Lookup, Observation,
    Page, ReadFailure, ReadStatus, Unreadable,
};
use crate::flow::{Flow, snapshot};
use crate::{IssueId, ProjectId, prelude::*};
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::ToString;
use serde::{Deserialize, Serialize};

/// # Errors
/// The tracker returned a different issue or a record for another project.
pub fn verify(
    request: &Lookup,
    record: &Observation,
) -> Result<(), ReadFailure> {
    if request.project != record.project()
        || request
            .expected
            .as_ref()
            .is_some_and(|id| id != record.id())
    {
        return Err(ReadFailure::IdentityConflict);
    }
    Ok(())
}

#[derive(Default)]
pub struct Scan {
    records: BTreeMap<IssueId, Observation>,
    continuations: BTreeSet<String>,
}

impl Scan {
    /// # Errors
    /// The tracker mixed projects, repeated an identity, or returned a continuation loop.
    pub fn append(
        &mut self,
        project: ProjectId,
        page: Page,
    ) -> Result<Option<String>, ReadFailure> {
        for record in page.records {
            if record.project() != project
                || self.records.insert(record.id().clone(), record).is_some()
            {
                return Err(ReadFailure::InvalidResponse);
            }
        }
        if page
            .next
            .as_ref()
            .is_some_and(|cursor| !self.continuations.insert(cursor.clone()))
        {
            return Err(ReadFailure::InvalidResponse);
        }
        Ok(page.next)
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Collection {
    list: Option<Membership>,
    known: BTreeMap<IssueId, Seen>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Membership {
    generation: u64,
    when: u64,
    ids: Vec<IssueId>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Seen {
    record: Observation,
    generation: u64,
    when: u64,
}

impl Collection {
    /// # Errors
    /// A locator in the completed scan identifies different issues.
    pub fn publish(
        &mut self,
        scan: Scan,
        generation: u64,
        when: u64,
    ) -> Result<(), ReadFailure> {
        if self
            .list
            .as_ref()
            .is_some_and(|old| old.generation >= generation)
        {
            return Ok(());
        }
        let mut references = BTreeMap::new();
        let known = self.known.values().map(|seen| &seen.record);
        for record in known.chain(scan.records.values()) {
            if let Some(address) = &record.locator
                && references
                    .insert(&address.reference, record.id())
                    .is_some_and(|old| old != record.id())
            {
                return Err(ReadFailure::IdentityConflict);
            }
        }
        self.list = Some(Membership {
            generation,
            when,
            ids: scan.records.keys().cloned().collect(),
        });
        for record in scan.records.into_values() {
            self.remember(record, generation, when);
        }
        Ok(())
    }

    /// # Errors
    /// A concurrent refresh observed another identity at the requested or returned locator.
    pub fn accept(
        &mut self,
        lookup: &Lookup,
        record: Observation,
        generation: u64,
        when: u64,
    ) -> Result<IssueId, ReadFailure> {
        let returned = record
            .locator
            .as_ref()
            .map(|address| address.reference.as_str());
        for reference in [Some(lookup.reference.as_str()), returned]
            .into_iter()
            .flatten()
        {
            verify(&self.lookup(lookup.project, reference)?, &record)?;
        }
        let id = record.id().clone();
        self.remember(record, generation, when);
        Ok(id)
    }

    fn remember(
        &mut self,
        mut record: Observation,
        generation: u64,
        when: u64,
    ) {
        if let Some(previous) = self.known.get(record.id()) {
            if previous.generation >= generation {
                return;
            }
            if let IssueState::Deleted(deleted)
            | IssueState::Unavailable(deleted) = &mut record.state
            {
                deleted.retain_text(&previous.record);
                if record.locator.is_none() {
                    record.locator.clone_from(&previous.record.locator);
                }
            }
        }
        self.known.insert(
            record.id().clone(),
            Seen {
                record,
                generation,
                when,
            },
        );
    }

    #[must_use]
    pub fn listing(
        &self,
        flow: &Flow,
        fresh: bool,
        failure: Option<ReadFailure>,
    ) -> IssueListing {
        let when = self.list.as_ref().map(|list| list.when);
        let status = ReadStatus::observed(when, fresh, failure);
        let records = self.evaluated(flow, None);
        IssueListing { records, status }
    }

    fn evaluated(
        &self,
        flow: &Flow,
        subject: Option<&IssueId>,
    ) -> Vec<Observation> {
        let listed = self.list.iter().flat_map(|list| list.ids.iter());
        let ids: BTreeSet<_> = listed.chain(subject).collect();
        let records = ids
            .into_iter()
            .filter_map(|id| self.known.get(id))
            .map(|seen| seen.record.clone())
            .collect();
        snapshot::project_issues(flow, records)
    }

    /// # Errors
    /// Multiple known identities claim the same display reference.
    pub fn lookup(
        &self,
        project: ProjectId,
        selector: &str,
    ) -> Result<Lookup, ReadFailure> {
        let id = IssueId::parse(selector);
        let known = if let Some(key) = &id {
            self.known.get(key)
        } else {
            let mut matches = self.known.values().filter(|seen| {
                seen.record
                    .locator
                    .as_ref()
                    .is_some_and(|address| address.reference == selector)
            });
            let first = matches.next();
            if matches.next().is_some() {
                return Err(ReadFailure::IdentityConflict);
            }
            first
        };
        let expected = known.map(|seen| seen.record.id().clone()).or(id);
        let address = known.and_then(|seen| seen.record.locator.as_ref());
        Ok(Lookup {
            project,
            expected,
            reference: address.map_or_else(
                || String::from(selector),
                |locator| locator.reference.clone(),
            ),
        })
    }

    #[must_use]
    pub fn failed_read(
        &self,
        flow: &Flow,
        lookup: &Lookup,
        error: ReadFailure,
    ) -> IssueReading {
        let current = self
            .lookup(lookup.project, &lookup.reference)
            .ok()
            .and_then(|request| request.expected);
        self.reading(
            flow,
            lookup.expected.as_ref().or(current.as_ref()),
            false,
            Some(error),
        )
    }

    #[must_use]
    pub fn reading(
        &self,
        flow: &Flow,
        id: Option<&IssueId>,
        fresh: bool,
        failure: Option<ReadFailure>,
    ) -> IssueReading {
        let seen = id.and_then(|key| self.known.get(key));
        let mut record = seen.and_then(|item| {
            self.evaluated(flow, id)
                .into_iter()
                .find(|record| record.id() == item.record.id())
        });
        if let Some(observation) = &mut record
            && failure == Some(ReadFailure::Unavailable)
        {
            observation.inaccessible();
        }
        let status =
            ReadStatus::observed(seen.map(|item| item.when), fresh, failure);
        IssueReading { record, status }
    }
}

impl ReadStatus {
    const fn observed(
        when: Option<u64>,
        fresh: bool,
        failure: Option<ReadFailure>,
    ) -> Self {
        let freshness = match (when, fresh) {
            (None, _) => Freshness::Unknown,
            (Some(_), true) => Freshness::Fresh,
            (Some(_), false) => Freshness::Stale,
        };
        Self {
            freshness,
            complete: when.is_some(),
            observed_at: when,
            failure,
        }
    }
}

impl Unreadable {
    fn retain_text(&mut self, previous: &Observation) {
        let (title, body) = match &previous.state {
            IssueState::Valid { issue, body } => {
                (Some(issue.title.clone()), Some(body.text()))
            }
            IssueState::Invalid(old)
            | IssueState::Unavailable(old)
            | IssueState::Deleted(old) => (old.title.clone(), old.body.clone()),
        };
        self.title = self.title.take().or(title);
        self.body = self.body.take().or(body);
        self.created = self.created.or_else(|| previous.created());
    }
}

impl Observation {
    fn inaccessible(&mut self) {
        let mut record = Unreadable {
            id: self.id().clone(),
            project: self.project(),
            title: None,
            body: None,
            created: None,
            reason: ReadFailure::Unavailable.to_string(),
        };
        record.retain_text(self);
        let incomplete = super::Coverage::Incomplete {
            reason: record.reason.clone(),
        };
        self.completeness.fields = incomplete.clone();
        self.completeness.notes = incomplete.clone();
        self.completeness
            .relations
            .values_mut()
            .for_each(|coverage| coverage.clone_from(&incomplete));
        self.state = IssueState::Unavailable(record);
    }
}
