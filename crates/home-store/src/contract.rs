use std::fmt::{Debug, Display};

use home::canvas::{
    Block, Broken, Canvas, CanvasEntry, CanvasFile, CanvasKey, CanvasName,
    Held, Stray, StrayReason,
};
use home::{IssueId, ProjectId};

use crate::{CanvasStore, StoreError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plant {
    Unnamed { project: ProjectId, issue: IssueId },
    Folder { project: ProjectId, issue: IssueId },
    NotAnIssue(ProjectId),
}

/// # Errors
///
/// The clause the store broke.
pub fn contract<S: CanvasStore>(
    store: &mut S,
    spoil: impl FnOnce(&mut S, &CanvasKey),
    plant: impl FnMut(&mut S, &Plant) -> String,
) -> Result<(), String> {
    keeps(store)?;
    reports_strays(store, spoil, plant)
}

fn keeps(store: &mut dyn CanvasStore) -> Result<(), String> {
    let (alpha, beta) = (ProjectId::new(1), ProjectId::new(2));
    let (one, two) = (IssueId::new(1, 0), IssueId::new(2, 0));
    let design = key(alpha, one, "design")?;
    let plan = key(alpha, one, "a-plan")?;
    let other = key(alpha, two, "design")?;
    let elsewhere = key(beta, one, "design")?;
    same(
        "get of a missing key is none",
        &store.get(&design).map_err(shown)?,
        &None,
    )?;
    let first = file(1, "first")?;
    store.put(&design, &first).map_err(shown)?;
    same(
        "get returns what put kept",
        &store.get(&design).map_err(shown)?,
        &Some(first),
    )?;
    let second = file(2, "second")?;
    store.put(&design, &second).map_err(shown)?;
    same(
        "put again under a key replaces what it kept",
        &store.get(&design).map_err(shown)?,
        &Some(second.clone()),
    )?;
    let third = file(3, "third")?;
    [&plan, &other, &elsewhere]
        .into_iter()
        .try_for_each(|at| store.put(at, &third))
        .map_err(shown)?;
    same(
        "list gives each canvas of the issue by name, with its time and signer",
        &seen(listed(store.list(&alpha, &one).map_err(shown)?)),
        &(
            vec![
                Ok(CanvasEntry::of(plan.name.clone(), &third)),
                Ok(CanvasEntry::of(design.name.clone(), &second)),
            ],
            Vec::new(),
        ),
    )?;
    same(
        "get of a name the issue has no canvas of is none",
        &store.get(&key(beta, two, "design")?).map_err(shown)?,
        &None,
    )?;
    same(
        "all gives every key in key order",
        &seen(store.all().map_err(shown)?),
        &(vec![plan, design, other, elsewhere], Vec::new()),
    )
}

fn reports_strays<S: CanvasStore>(
    store: &mut S,
    spoil: impl FnOnce(&mut S, &CanvasKey),
    mut plant: impl FnMut(&mut S, &Plant) -> String,
) -> Result<(), String> {
    let (alpha, beta) = (ProjectId::new(1), ProjectId::new(2));
    let (one, two) = (IssueId::new(1, 0), IssueId::new(2, 0));
    let design = key(alpha, one, "design")?;
    let plan = key(alpha, one, "a-plan")?;
    let broken = key(alpha, one, "broken")?;
    spoil(store, &broken);
    let unnamed = plant(
        store,
        &Plant::Unnamed {
            project: alpha,
            issue: one,
        },
    );
    let folder = plant(
        store,
        &Plant::Folder {
            project: alpha,
            issue: one,
        },
    );
    let unissued = plant(store, &Plant::NotAnIssue(alpha));
    same(
        "list gives an issue's canvases by name, a broken one's too, beside what isn't one, with where it is and why",
        &seen(listed(store.list(&alpha, &one).map_err(shown)?)),
        &want(
            vec![
                Ok(CanvasEntry::of(plan.name.clone(), &file(3, "third")?)),
                Err(broken.name.clone()),
                Ok(CanvasEntry::of(design.name.clone(), &file(2, "second")?)),
            ],
            vec![(unnamed.clone(), "unnamed"), (folder.clone(), "folder")],
        ),
    )?;
    same(
        "all gives every key, a broken canvas's too, beside what can't be a key",
        &seen(store.all().map_err(shown)?),
        &want(
            vec![
                plan,
                broken.clone(),
                design,
                key(alpha, two, "design")?,
                key(beta, one, "design")?,
            ],
            vec![
                (unnamed, "unnamed"),
                (folder, "folder"),
                (unissued, "not an issue"),
            ],
        ),
    )?;
    match store.get(&broken) {
        Err(StoreError::Invalid { key, .. }) if key == broken => Ok(()),
        got => Err(format!(
            "get of a broken canvas is invalid under its key: got {got:?}"
        )),
    }
}

type Seen<T> = (Vec<T>, Vec<(String, &'static str)>);

fn want<T>(
    canvases: Vec<T>,
    mut strays: Vec<(String, &'static str)>,
) -> Seen<T> {
    strays.sort();
    (canvases, strays)
}

fn seen<T>(held: Held<T>) -> Seen<T> {
    let mut strays: Vec<(String, &'static str)> = held
        .strays
        .into_iter()
        .map(|Stray { place, reason }| {
            let why = match reason {
                StrayReason::Unnamed => "unnamed",
                StrayReason::Folder => "folder",
                StrayReason::NotAnIssue => "not an issue",
            };
            (place, why)
        })
        .collect();
    strays.sort();
    (held.canvases, strays)
}

fn listed(
    held: Held<Result<CanvasEntry, Broken>>,
) -> Held<Result<CanvasEntry, CanvasName>> {
    Held {
        canvases: held
            .canvases
            .into_iter()
            .map(|kept| kept.map_err(|broken| broken.name))
            .collect(),
        strays: held.strays,
    }
}

fn key(
    project: ProjectId,
    issue: IssueId,
    name: &str,
) -> Result<CanvasKey, String> {
    Ok(CanvasKey {
        project,
        issue,
        name: CanvasName::parse(name).map_err(shown)?,
    })
}

fn file(written: u64, text: &str) -> Result<CanvasFile, String> {
    Ok(CanvasFile {
        written,
        by: format!("host·{text}"),
        canvas: Canvas::new(
            format!("The {text} canvas"),
            vec![Block::Prose {
                text: text.to_owned(),
            }],
        )
        .map_err(shown)?,
    })
}

fn same<T: PartialEq + Debug>(
    clause: &str,
    got: &T,
    want: &T,
) -> Result<(), String> {
    (got == want)
        .then_some(())
        .ok_or_else(|| format!("{clause}: got {got:?}, want {want:?}"))
}

fn shown(error: impl Display) -> String {
    error.to_string()
}
