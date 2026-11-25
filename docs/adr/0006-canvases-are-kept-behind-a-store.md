# Canvases are kept behind a store

The core names a canvas by its project, issue and name. A `CanvasStore` trait in the shell keeps it, and the home folder in git is the first store. The owner may host canvases elsewhere later, and then only a new store should change, not the core, the `home` command, the host or the app. With one store today, the trait costs one indirection and a contract suite that every store passes.

## Rejected

- **Canvases as files in the home, read where they're needed**, as issues are. It's less code now, but every reader would know the path, and moving canvases later would touch every reader.
