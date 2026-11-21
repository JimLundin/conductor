use std::ffi::{OsStr, OsString};
use std::io::{self, ErrorKind, Read, Write};
use std::path::{Component, Path};

use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::ambient_authority;
use cap_std::fs::{Dir, Metadata, OpenOptions};
use home::sync::{IssueLeaf, IssueParent, IssuePath, issue_leaf, issue_parent};

pub(super) struct IssueFile {
    // Windows capability operations may resolve a handle to a path. Retain
    // every directory handle so their non-delete sharing mode pins the path.
    directories: Vec<Dir>,
    leaf: OsString,
}

impl IssueFile {
    fn open(
        clone: &Path,
        relative: &Path,
        create: bool,
    ) -> io::Result<Option<Self>> {
        let path = IssuePath::checked(
            relative.components().map(normal_name).collect(),
        )
        .map_err(|_| unsafe_path())?;
        let checkout = clone.file_name().ok_or_else(unsafe_path)?;
        let parent = clone.parent().ok_or_else(unsafe_path)?;
        let home = Dir::open_ambient_dir(parent, ambient_authority())?;
        let root = open_dir(&home, Path::new(checkout))?;
        let mut directories = vec![home, root];
        for name in path.parents {
            let directory = directories.last().ok_or_else(unsafe_path)?;
            let Some(next) = descend(directory, Path::new(name), create)?
            else {
                return Ok(None);
            };
            directories.push(next);
        }
        let directory = directories.last().ok_or_else(unsafe_path)?;
        check_leaf(directory, Path::new(path.leaf))?;
        Ok(Some(Self {
            directories,
            leaf: path.leaf.to_os_string(),
        }))
    }

    fn read(&self) -> io::Result<Option<String>> {
        let opened = optional(
            self.directory()?.open_with(&self.leaf, &read_options()),
            ErrorKind::NotFound,
        )?;
        let Some(mut file) = opened else {
            return Ok(None);
        };
        let metadata = file.metadata()?;
        issue_leaf(Some(file_kind(&metadata))).map_err(|_| unsafe_path())?;
        let mut contents = String::new();
        file.read_to_string(&mut contents)?;
        Ok(Some(contents))
    }

    fn write(&self, contents: &str) -> io::Result<()> {
        let directory = self.directory()?;
        let next = format!(".conductor-{}.new", ulid::Ulid::new());
        let mut file = directory.open_with(
            &next,
            OpenOptions::new().write(true).create_new(true),
        )?;
        let result = (|| {
            file.write_all(contents.as_bytes())?;
            drop(file);
            check_leaf(directory, Path::new(&self.leaf))?;
            dir_ops::replace(directory, Path::new(&next), Path::new(&self.leaf))
        })();
        if result.is_err() {
            let _ = dir_ops::remove(directory, Path::new(&next));
        }
        result
    }

    fn directory(&self) -> io::Result<&Dir> {
        self.directories.last().ok_or_else(unsafe_path)
    }
}

fn read_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.read(true).follow(FollowSymlinks::No);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;

        options.custom_flags(libc::O_NONBLOCK);
    }
    options
}

fn open_dir(directory: &Dir, name: &Path) -> io::Result<Dir> {
    let opened = directory.open_dir_nofollow(name)?;
    if reparse_point(&opened.dir_metadata()?) {
        return Err(unsafe_path());
    }
    Ok(opened)
}

fn descend(
    directory: &Dir,
    name: &Path,
    create: bool,
) -> io::Result<Option<Dir>> {
    let opened = optional(open_dir(directory, name), ErrorKind::NotFound)?;
    match issue_parent(opened.is_some(), create) {
        IssueParent::Opened => Ok(opened),
        IssueParent::Absent => Ok(None),
        IssueParent::Create => {
            let _ = optional(
                dir_ops::create(directory, name),
                ErrorKind::AlreadyExists,
            )?;
            open_dir(directory, name).map(Some)
        }
    }
}

fn check_leaf(directory: &Dir, name: &Path) -> io::Result<()> {
    let found =
        optional(directory.symlink_metadata(name), ErrorKind::NotFound)?;
    issue_leaf(found.as_ref().map(file_kind)).map_err(|_| unsafe_path())
}

fn file_kind(metadata: &Metadata) -> IssueLeaf {
    IssueLeaf {
        regular: metadata.is_file(),
        reparse_point: reparse_point(metadata),
    }
}

const fn normal_name(part: Component<'_>) -> Option<&OsStr> {
    match part {
        Component::Normal(name) => Some(name),
        _ => None,
    }
}

fn optional<T>(
    result: io::Result<T>,
    absent: ErrorKind,
) -> io::Result<Option<T>> {
    result.map(Some).or_else(|error| {
        if error.kind() == absent {
            Ok(None)
        } else {
            Err(error)
        }
    })
}

#[cfg(windows)]
fn reparse_point(metadata: &Metadata) -> bool {
    use cap_std::fs::MetadataExt;

    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
const fn reparse_point(_metadata: &Metadata) -> bool {
    false
}

#[cfg(not(windows))]
mod dir_ops {
    use super::{Dir, Path, io};

    pub(super) fn create(dir: &Dir, leaf: &Path) -> io::Result<()> {
        dir.create_dir(leaf)
    }

    pub(super) fn replace(
        dir: &Dir,
        source: &Path,
        target: &Path,
    ) -> io::Result<()> {
        dir.rename(source, dir, target)
    }

    pub(super) fn remove(dir: &Dir, leaf: &Path) -> io::Result<()> {
        dir.remove_file(leaf)
    }
}

#[cfg(windows)]
mod dir_ops {
    use std::path::PathBuf;

    use super::{Dir, Path, io, unsafe_path};

    // All ancestors stay open without delete sharing. Resolve the pinned
    // handle and keep its verbatim prefix, including `\\?\UNC\` on shares.
    fn directory_path(dir: &Dir) -> io::Result<PathBuf> {
        let file = dir.try_clone()?.into_std_file();
        absolute(winx::file::get_file_path(&file)?)
    }

    fn absolute(path: PathBuf) -> io::Result<PathBuf> {
        if path.is_absolute() {
            Ok(path)
        } else {
            Err(unsafe_path())
        }
    }

    pub(super) fn create(dir: &Dir, leaf: &Path) -> io::Result<()> {
        std::fs::create_dir(directory_path(dir)?.join(leaf))
    }

    pub(super) fn replace(
        dir: &Dir,
        source: &Path,
        target: &Path,
    ) -> io::Result<()> {
        let parent = directory_path(dir)?;
        std::fs::rename(parent.join(source), parent.join(target))
    }

    pub(super) fn remove(dir: &Dir, leaf: &Path) -> io::Result<()> {
        std::fs::remove_file(directory_path(dir)?.join(leaf))
    }

    #[cfg(test)]
    #[test]
    fn windows_handle_paths_keep_absolute_drive_and_share_prefixes()
    -> io::Result<()> {
        for path in [
            r"\\?\C:\conductor\home-repo",
            r"\\?\UNC\server\share\home-repo",
        ] {
            assert_eq!(absolute(PathBuf::from(path))?, PathBuf::from(path));
        }
        assert!(absolute(PathBuf::from(r"UNC\server\share")).is_err());
        Ok(())
    }
}

fn unsafe_path() -> io::Error {
    io::Error::new(ErrorKind::InvalidInput, "unsafe issue path")
}

pub(super) fn read_issue(
    clone: &Path,
    relative: &str,
) -> Result<Option<String>, String> {
    IssueFile::open(clone, Path::new(relative), false)
        .and_then(|target| target.map_or(Ok(None), |file| file.read()))
        .map_err(|error| format!("could not read issue {relative}: {error}"))
}

pub(super) fn write_issue(
    clone: &Path,
    relative: &str,
    contents: &str,
) -> Result<IssueFile, String> {
    let target = IssueFile::open(clone, Path::new(relative), true)
        .and_then(|target| target.ok_or_else(unsafe_path))
        .map_err(|error| format!("could not open issue {relative}: {error}"))?;
    target.write(contents).map_err(|error| {
        format!("could not write issue {relative}: {error}")
    })?;
    Ok(target)
}
