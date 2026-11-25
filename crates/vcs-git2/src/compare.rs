use anyhow::Context as _;
use git2::{Delta, Diff, DiffOptions, Oid, Patch, Repository};
use vcs::command::Comparison;
use vcs::plan::{FILE_LIMIT, PATCH_LIMIT, budget};
use vcs::{DiffChange, DiffExtent, FilePatch, SnapshotFile};

pub(super) fn revisions(
    repo: &Repository,
    previous: Oid,
    revision: Oid,
) -> anyhow::Result<Comparison> {
    let baseline = repo.find_commit(previous)?.tree()?;
    let current = repo.find_commit(revision)?.tree()?;
    let mut options = DiffOptions::new();
    options.ignore_submodules(false).include_typechange(true);
    let diff = repo.diff_tree_to_tree(
        Some(&baseline),
        Some(&current),
        Some(&mut options),
    )?;
    let count = diff.deltas().len();
    let counted = if count > FILE_LIMIT {
        DiffExtent::Truncated
    } else {
        DiffExtent::Complete
    };
    let list = diff
        .deltas()
        .take(FILE_LIMIT)
        .enumerate()
        .map(|(index, delta)| {
            let name = delta
                .new_file()
                .path_bytes()
                .context("snapshot entry has no path")?;
            Ok(SnapshotFile {
                path: std::str::from_utf8(name)
                    .context("non-UTF-8 snapshot path")?
                    .to_owned(),
                change: change(delta.status()),
                patch: patch(&diff, index)?,
            })
        })
        .collect::<anyhow::Result<_>>()?;
    let (files, extent) = budget(list, counted);
    Ok(Comparison { files, extent })
}

const fn change(kind: Delta) -> DiffChange {
    match kind {
        Delta::Added => DiffChange::Write,
        Delta::Deleted => DiffChange::Delete,
        _ => DiffChange::Edit,
    }
}

fn patch(diff: &Diff<'_>, index: usize) -> anyhow::Result<FilePatch> {
    let Some(mut patch) = Patch::from_diff(diff, index)? else {
        return Ok(FilePatch::Binary);
    };
    if patch.size(true, true, true) > PATCH_LIMIT {
        return Ok(FilePatch::TooLarge);
    }
    let buffer = patch.to_buf()?;
    Ok(std::str::from_utf8(&buffer).map_or_else(
        |reason| FilePatch::Unsupported {
            reason: reason.to_string(),
        },
        |text| FilePatch::Text(text.to_owned()),
    ))
}
