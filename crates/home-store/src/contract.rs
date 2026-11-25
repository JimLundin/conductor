use std::fmt::{Debug, Display};

use home::canvas::{
    Block, Canvas, CanvasEntry, CanvasFile, CanvasKey, CanvasName,
};
use home::{IssueId, ProjectId};

use crate::CanvasStore;

/// Runs the contract every [`CanvasStore`] keeps on `store`, which starts
/// empty, and says which clause it broke.
///
/// # Errors
///
/// The clause the store broke, with what it gave and what it should have.
pub fn contract(store: &dyn CanvasStore) -> Result<(), String> {
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
    let first = file(1, "first");
    store.put(&design, &first).map_err(shown)?;
    same(
        "get returns what put kept",
        &store.get(&design).map_err(shown)?,
        &Some(first),
    )?;
    let second = file(2, "second");
    store.put(&design, &second).map_err(shown)?;
    same(
        "put again under a key replaces what it kept",
        &store.get(&design).map_err(shown)?,
        &Some(second.clone()),
    )?;
    let third = file(3, "third");
    [&plan, &other, &elsewhere]
        .into_iter()
        .try_for_each(|at| store.put(at, &third))
        .map_err(shown)?;
    same(
        "list gives each canvas of the issue by name, with its time and signer",
        &store.list(&alpha, &one).map_err(shown)?,
        &vec![
            CanvasEntry::of(plan.name.clone(), &third),
            CanvasEntry::of(design.name.clone(), &second),
        ],
    )?;
    same(
        "get of a name the issue has no canvas of is none",
        &store.get(&key(beta, two, "design")?).map_err(shown)?,
        &None,
    )?;
    same(
        "all gives every key in key order",
        &store.all().map_err(shown)?,
        &vec![plan, design, other, elsewhere],
    )
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

fn file(written: u64, text: &str) -> CanvasFile {
    CanvasFile {
        written,
        by: format!("host·{text}"),
        canvas: Canvas {
            title: format!("The {text} canvas"),
            blocks: vec![Block::Prose {
                text: text.to_owned(),
            }],
        },
    }
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
