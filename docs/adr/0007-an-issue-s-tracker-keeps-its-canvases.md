# An issue's tracker keeps its canvases

Each issue tracker keeps the canvases of its own issues, behind the `CanvasStore` that ADR 0006 defines.

- **The Markdown tracker** keeps them in the home folder, as before.
- **GitHub** keeps each canvas as one comment on its issue. The comment opens with a hidden marker naming the canvas. Below the marker, people read the canvas as Markdown with Mermaid, and the canvas's data sits in a collapsed block.

The CLI, the daemon and the app find an issue and its canvases by its project and its issue ID through one resolver, and never ask which tracker a project uses.

An issue ID is a string unique within its tracker, such as a Markdown slug or a GitHub issue number. Outside the tracker, nothing reads meaning into it.

## Consequences

- A GitHub canvas write is a tracker write. It follows the backend contract of spec 0v265ymj: re-read before writing, an operation marker in a created comment, and no blind retry. So it needs GitHub to be reachable.
- GitHub caps a comment at 65,536 characters, so a canvas larger than that fails with a clear error.
- The `IssueId` type loses its authority and local slug, and the separate `#42` reference goes. Every command takes `<project> <id>`.

## Rejected

- **The home folder for every tracker's canvases.** It needs no new store, and the folder already accepts a qualified ID. But a GitHub issue's canvas would then live apart from its issue, unseen by anyone reading the issue on GitHub. The CLI would also have to treat one tracker's issues differently from another's.
- **A file in the GitHub repository.** It would need permission to write the repository's contents, beyond the tracker's Issues permission, and it would put the canvas in the code's history instead of on the issue.
