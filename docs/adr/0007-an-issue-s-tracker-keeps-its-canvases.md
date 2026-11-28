# An issue's tracker keeps its canvases

Each issue tracker keeps the canvases of its own issues, behind the `CanvasStore` that ADR 0006 defines.

- **The Markdown tracker** keeps them in the home folder, as before.
- **GitHub** keeps each canvas as one comment on its issue. The comment opens with a hidden marker naming the canvas. Below the marker, people read the canvas as Markdown with Mermaid, and the canvas's data sits in a collapsed block.

The CLI, the daemon and the app find an issue and its canvases by its project and its issue ID through one resolver, and never ask which tracker a project uses.

An issue ID is a value unique within its tracker. Each tracker has its own ID type: the Markdown tracker's is its slug, and GitHub's is the issue number. The core holds an ID as an opaque `IssueId`, which it compares, hashes and prints but never builds from text. Only the project's tracker reads an ID from text, at every edge: the CLI's `<project> <id>`, front matter, the `project-data` footer and the client link.

A relation names its target by two fields, the project and the ID. A target in the issue's own project leaves the project out.

## Consequences

- One selection, `project_runtime::tracker::Trackers`, picks each project's tracker from its configuration. The lookup, the refresh, the listing, the canvas store and the commit all go through it. A configured tracker that can't be built fails each command on its project and names the reason, and its canvases never go to the home folder.
- A GitHub canvas write is a tracker write. It follows the backend contract of spec 0v265ymj: re-read before writing, an operation marker in a created comment, and no blind retry. So it needs GitHub to be reachable.
- GitHub caps a comment at 65,536 characters, so a canvas larger than that fails with a clear error.
- The `IssueId` type loses its authority and local slug, and the separate `#42` reference goes. Every command takes `<project> <id>`.
- The `<project>/<id>` string form goes. A front matter or footer relation to another project's issue is a table, `{ project = "hyn162", id = "0trfecpc" }`, and relations stored in the old form no longer read.

## Rejected

- **The home folder for every tracker's canvases.** It needs no new store, and the folder already accepts a qualified ID. But a GitHub issue's canvas would then live apart from its issue, unseen by anyone reading the issue on GitHub. The CLI would also have to treat one tracker's issues differently from another's.
- **A file in the GitHub repository.** It would need permission to write the repository's contents, beyond the tracker's Issues permission, and it would put the canvas in the code's history instead of on the issue.
- **One string type for every tracker's IDs, with a `<project>/<id>` prefix for another project's issue.** A Markdown slug or a GitHub number never holds a slash, but another tracker's ID may, and the prefix parse would then read part of the ID as a project.
