//! Which project a folder belongs to, and what Git says about it.
//!
//! Sessions are grouped by project: the Git repository their folder is in,
//! or the folder itself when it is in none. [`detect`] answers that for one
//! folder.
//!
//! # How Git is read
//!
//! The metadata is read directly from the repository: `git` is never run.
//! Finding the repository takes a few `stat` calls per parent folder, and
//! the branch is the content of one small file (`HEAD`). That works the same
//! on every platform, needs no Git installation, and costs microseconds, so
//! it can be repeated often.
//!
//! What is understood:
//!
//! - **A repository**: a folder with a `.git` directory. The innermost one
//!   wins, so a repository nested in another is its own project, as is a
//!   **submodule** (whose `.git` is a file pointing into the parent's
//!   `.git/modules`).
//! - **A linked worktree** (`git worktree add`): its `.git` is a file
//!   pointing to `<repository>/.git/worktrees/<name>`. The project is the
//!   main repository, so that every worktree of a repository is grouped
//!   with it, and [`GitInfo::worktree`] names the worktree.
//! - **A bare repository**: the project is the repository folder itself,
//!   also for the worktrees attached to it.
//! - **The branch**: the branch `HEAD` points to, or the abbreviated commit
//!   when `HEAD` is detached.
//!
//! What is not: a repository that stores its references in the `reftable`
//! format keeps a placeholder in `HEAD`, so its branch is reported as
//! unknown ([`GitInfo::branch`] and [`GitInfo::commit`] both `None`).
//! `GIT_DIR`, `GIT_WORK_TREE` and `GIT_CEILING_DIRECTORIES` are ignored.
//!
//! A folder that does not exist or cannot be read is not an error: the
//! search simply finds nothing there and goes on with its parents.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Most that is read of `HEAD` and of the other small files of Git. They
/// hold one line; a larger file is something else.
const MAX_METADATA_BYTES: u64 = 4096;

/// Length a detached commit is abbreviated to, as `git` does by default.
const SHORT_COMMIT_LEN: usize = 7;

/// What groups sessions: a Git repository or a plain folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProjectKind {
    /// A Git repository.
    Git,
    /// A folder that is in no repository.
    Folder,
}

/// The project a session belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    /// Identifies the project: sessions with the same key are grouped. It
    /// is the path of [`root`](Self::root), as text.
    pub key: String,
    /// The name to show: the name of the root folder.
    pub name: String,
    /// The root of the repository, or the folder itself.
    #[serde(serialize_with = "super::lossy_path")]
    pub root: PathBuf,
    /// Whether the project is a repository.
    pub kind: ProjectKind,
}

impl Project {
    /// The project of a folder that is in no repository. Nothing is read
    /// from the disk.
    #[must_use]
    pub fn folder(path: &Path) -> Self {
        Self {
            key: path.to_string_lossy().into_owned(),
            name: display_name(path),
            root: path.to_owned(),
            kind: ProjectKind::Folder,
        }
    }
}

/// What Git says about the folder of a session.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitInfo {
    /// The branch that is checked out. `None` when `HEAD` is detached or
    /// could not be read.
    pub branch: Option<String>,
    /// The abbreviated commit, when `HEAD` is detached.
    pub commit: Option<String>,
    /// The name of the folder of the linked worktree the session is in.
    /// `None` in the main working tree of a repository.
    pub worktree: Option<String>,
}

/// The answer of [`detect`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detected {
    /// The project the folder belongs to.
    pub project: Project,
    /// What Git says, when the folder is in a repository.
    pub git: Option<GitInfo>,
}

/// Finds the project of `folder` and its Git state. See the
/// [module documentation](self).
///
/// This reads the disk, so it can take long on a network drive that hangs:
/// do not call it from a thread that must stay responsive.
#[must_use]
pub fn detect(folder: &Path) -> Detected {
    let folder = normalize(folder);
    match discover(&folder) {
        Some(repository) => {
            let root = repository.project_root();
            let name = display_name(&root);
            let name = match name.strip_suffix(".git") {
                Some(stem) if !stem.is_empty() && !repository.has_work_tree_at_root() => {
                    stem.to_owned()
                }
                _ => name,
            };
            Detected {
                project: Project {
                    key: root.to_string_lossy().into_owned(),
                    name,
                    root,
                    kind: ProjectKind::Git,
                },
                git: Some(repository.info()),
            }
        }
        None => Detected {
            project: Project::folder(&folder),
            git: None,
        },
    }
}

/// The path of an existing folder with its symbolic links resolved, so that
/// two spellings of one folder give one project. A path that cannot be
/// resolved (it does not exist, or may not be read) is only made absolute.
///
/// On Windows the result keeps the usual `C:\...` form instead of the
/// `\\?\C:\...` form the system resolves to.
#[must_use]
pub fn normalize(path: &Path) -> PathBuf {
    match fs::canonicalize(path) {
        Ok(resolved) => simplify_verbatim(resolved),
        Err(_) => std::path::absolute(path).unwrap_or_else(|_| path.to_owned()),
    }
}

/// Turns `\\?\C:\dir` into `C:\dir`. Other verbatim paths (`\\?\UNC\...`)
/// have no shorter form that is always equivalent and are left alone.
fn simplify_verbatim(path: PathBuf) -> PathBuf {
    if !cfg!(windows) {
        return path;
    }
    let Some(text) = path.to_str() else {
        return path;
    };
    match text.strip_prefix(r"\\?\") {
        Some(rest)
            if rest.as_bytes().get(1) == Some(&b':')
                && rest.as_bytes().first().is_some_and(u8::is_ascii_alphabetic) =>
        {
            PathBuf::from(rest)
        }
        _ => path,
    }
}

/// The name of a folder, for display: its last component, or the whole path
/// for a root.
fn display_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.to_string_lossy().into_owned(),
        |name| name.to_string_lossy().into_owned(),
    )
}

/// A repository found on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Repository {
    /// The folder whose `.git` was found. `None` when the search started
    /// inside a Git directory: a bare repository, or the `.git` directory
    /// of an ordinary one.
    work_tree: Option<PathBuf>,
    /// Where `HEAD` is: `.git`, or `.git/worktrees/<name>` for a linked
    /// worktree, or `.git/modules/<name>` for a submodule.
    git_dir: PathBuf,
    /// The Git directory shared by the worktrees of the repository. `None`
    /// unless this is a linked worktree.
    common_dir: Option<PathBuf>,
}

impl Repository {
    /// The folder that stands for the repository as a project.
    fn project_root(&self) -> PathBuf {
        match (&self.common_dir, &self.work_tree) {
            // A linked worktree belongs to the repository it was added to.
            (Some(common), _) => enclosing_work_tree(common),
            (None, Some(work_tree)) => work_tree.clone(),
            (None, None) => enclosing_work_tree(&self.git_dir),
        }
    }

    /// Whether the project root is a working tree, as opposed to a bare
    /// repository.
    fn has_work_tree_at_root(&self) -> bool {
        let git_dir = self.common_dir.as_ref().unwrap_or(&self.git_dir);
        self.common_dir.is_none() && self.work_tree.is_some()
            || git_dir.file_name().is_some_and(|name| name == ".git")
    }

    fn info(&self) -> GitInfo {
        let (branch, commit) = read_head(&self.git_dir.join("HEAD"));
        let worktree = match (&self.common_dir, &self.work_tree) {
            (Some(_), Some(work_tree)) => Some(display_name(work_tree)),
            _ => None,
        };
        GitInfo {
            branch,
            commit,
            worktree,
        }
    }
}

/// The working tree a Git directory belongs to: the parent of a directory
/// named `.git`, and the directory itself otherwise (a bare repository).
fn enclosing_work_tree(git_dir: &Path) -> PathBuf {
    match (git_dir.file_name(), git_dir.parent()) {
        (Some(name), Some(parent)) if name == ".git" => parent.to_owned(),
        _ => git_dir.to_owned(),
    }
}

/// Searches `start` and its parents for a repository, innermost first.
fn discover(start: &Path) -> Option<Repository> {
    for dir in start.ancestors() {
        let dot_git = dir.join(".git");
        match fs::metadata(&dot_git) {
            Ok(metadata) if metadata.is_dir() => {
                if is_git_dir(&dot_git) {
                    return Some(Repository {
                        work_tree: Some(dir.to_owned()),
                        git_dir: dot_git,
                        common_dir: None,
                    });
                }
            }
            Ok(metadata) if metadata.is_file() => {
                if let Some(repository) = follow_git_file(dir, &dot_git) {
                    return Some(repository);
                }
            }
            _ => {}
        }
        // The folder itself may be a Git directory: a bare repository, or
        // somewhere inside `.git`.
        if is_git_dir(dir) {
            return Some(Repository {
                work_tree: None,
                git_dir: dir.to_owned(),
                common_dir: None,
            });
        }
    }
    None
}

/// Whether `dir` looks like the Git directory of a repository, by the same
/// signs `git` checks.
fn is_git_dir(dir: &Path) -> bool {
    dir.join("HEAD").is_file() && dir.join("objects").is_dir() && dir.join("refs").is_dir()
}

/// Follows a `.git` file (`gitdir: <path>`), as found in a linked worktree
/// or a submodule.
fn follow_git_file(work_tree: &Path, dot_git: &Path) -> Option<Repository> {
    let content = read_small(dot_git)?;
    let target = content.trim().strip_prefix("gitdir:")?.trim();
    if target.is_empty() {
        return None;
    }
    let git_dir = resolve(work_tree, Path::new(target));
    if !git_dir.join("HEAD").is_file() {
        return None;
    }
    // Only a linked worktree has a `commondir` file, naming the Git
    // directory of the repository it belongs to.
    let common_dir = read_small(&git_dir.join("commondir"))
        .map(|content| content.trim().to_owned())
        .filter(|content| !content.is_empty())
        .map(|content| resolve(&git_dir, Path::new(&content)));
    Some(Repository {
        work_tree: Some(work_tree.to_owned()),
        git_dir,
        common_dir,
    })
}

/// `path` relative to `base`, without `..` left in it when the result
/// exists.
fn resolve(base: &Path, path: &Path) -> PathBuf {
    let joined = if path.is_absolute() {
        path.to_owned()
    } else {
        base.join(path)
    };
    normalize(&joined)
}

/// Reads a small text file. `None` if it cannot be read or is not text.
fn read_small(path: &Path) -> Option<String> {
    let file = fs::File::open(path).ok()?;
    let mut content = String::new();
    file.take(MAX_METADATA_BYTES)
        .read_to_string(&mut content)
        .ok()?;
    Some(content)
}

/// The branch and the abbreviated commit a `HEAD` file names. At most one
/// of them is set.
fn read_head(head: &Path) -> (Option<String>, Option<String>) {
    let Some(content) = read_small(head) else {
        return (None, None);
    };
    let content = content.trim();
    if let Some(reference) = content.strip_prefix("ref:") {
        let reference = reference.trim();
        // What a repository in the reftable format keeps there.
        if reference == "refs/heads/.invalid" || reference.is_empty() {
            return (None, None);
        }
        let branch = reference.strip_prefix("refs/heads/").unwrap_or(reference);
        return (Some(branch.to_owned()), None);
    }
    let is_commit =
        matches!(content.len(), 40 | 64) && content.bytes().all(|b| b.is_ascii_hexdigit());
    if is_commit {
        (None, Some(content[..SHORT_COMMIT_LEN].to_ascii_lowercase()))
    } else {
        (None, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Writes the files that make a folder a repository, as `git init`
    /// does, without running `git`.
    fn fake_repository(git_dir: &Path, head: &str) {
        fs::create_dir_all(git_dir.join("objects")).unwrap();
        fs::create_dir_all(git_dir.join("refs").join("heads")).unwrap();
        fs::write(git_dir.join("HEAD"), head).unwrap();
    }

    #[test]
    fn a_folder_outside_any_repository_is_its_own_project() {
        let dir = tempfile::tempdir().unwrap();
        let folder = normalize(dir.path()).join("notes");
        fs::create_dir(&folder).unwrap();

        let detected = detect(&folder);

        assert_eq!(detected.git, None);
        assert_eq!(detected.project.kind, ProjectKind::Folder);
        assert_eq!(detected.project.name, "notes");
        assert_eq!(detected.project.root, folder);
        assert_eq!(detected.project.key, folder.to_string_lossy());
    }

    #[test]
    fn the_branch_is_read_from_head() {
        let dir = tempfile::tempdir().unwrap();
        let root = normalize(dir.path()).join("app");
        fake_repository(&root.join(".git"), "ref: refs/heads/feature/login\n");
        let nested = root.join("src").join("deep");
        fs::create_dir_all(&nested).unwrap();

        let detected = detect(&nested);

        assert_eq!(detected.project.kind, ProjectKind::Git);
        assert_eq!(detected.project.root, root);
        assert_eq!(detected.project.name, "app");
        assert_eq!(
            detected.git,
            Some(GitInfo {
                branch: Some("feature/login".to_owned()),
                commit: None,
                worktree: None,
            })
        );
    }

    #[test]
    fn a_detached_head_gives_the_abbreviated_commit() {
        let dir = tempfile::tempdir().unwrap();
        let root = normalize(dir.path());
        fake_repository(
            &root.join(".git"),
            "0123456789ABCDEF0123456789abcdef01234567\n",
        );

        let git = detect(&root).git.unwrap();

        assert_eq!(git.branch, None);
        assert_eq!(git.commit.as_deref(), Some("0123456"));
    }

    #[test]
    fn an_unreadable_head_gives_neither_branch_nor_commit() {
        let dir = tempfile::tempdir().unwrap();
        let root = normalize(dir.path());
        for head in ["", "garbage", "ref: refs/heads/.invalid\n", "ref:\n"] {
            fake_repository(&root.join(".git"), head);

            let detected = detect(&root);

            assert_eq!(detected.project.kind, ProjectKind::Git, "{head:?}");
            assert_eq!(detected.git, Some(GitInfo::default()), "{head:?}");
        }
    }

    #[test]
    fn a_folder_named_dot_git_that_is_no_repository_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let root = normalize(dir.path());
        fs::create_dir(root.join(".git")).unwrap();
        fs::write(root.join(".git").join("HEAD"), "ref: refs/heads/main\n").unwrap();

        // No `objects` and no `refs`: not a repository.
        assert_eq!(detect(&root).project.kind, ProjectKind::Folder);
    }

    #[test]
    fn a_dot_git_file_that_leads_nowhere_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let root = normalize(dir.path());
        for content in ["gitdir: ../gone\n", "gitdir:\n", "something else\n", ""] {
            fs::write(root.join(".git"), content).unwrap();

            assert_eq!(
                detect(&root).project.kind,
                ProjectKind::Folder,
                "{content:?}"
            );
        }
    }

    #[test]
    fn a_folder_that_does_not_exist_is_a_project_of_its_own_name() {
        let dir = tempfile::tempdir().unwrap();
        let gone = normalize(dir.path()).join("gone").join("deeper");

        let detected = detect(&gone);

        assert_eq!(detected.project.kind, ProjectKind::Folder);
        assert_eq!(detected.project.name, "deeper");
        assert_eq!(detected.project.root, gone);
        assert_eq!(detected.git, None);
    }

    #[test]
    fn a_missing_folder_inside_a_repository_still_belongs_to_it() {
        let dir = tempfile::tempdir().unwrap();
        let root = normalize(dir.path());
        fake_repository(&root.join(".git"), "ref: refs/heads/main\n");

        let detected = detect(&root.join("removed").join("folder"));

        assert_eq!(detected.project.root, root);
        assert_eq!(detected.git.unwrap().branch.as_deref(), Some("main"));
    }

    #[test]
    fn a_relative_path_is_made_absolute() {
        // Whatever the project is (the tests may well run inside a
        // repository), its root is an absolute path.
        let detected = detect(Path::new("surely-not-an-existing-folder-of-termx"));

        assert!(detected.project.root.is_absolute());
        assert!(Path::new(&detected.project.key).is_absolute());
    }

    #[test]
    fn the_root_of_the_file_system_has_a_name() {
        let root = normalize(Path::new(if cfg!(windows) { r"C:\" } else { "/" }));

        assert!(!Project::folder(&root).name.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn a_folder_that_may_not_be_read_is_not_an_error() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let root = normalize(dir.path());
        let locked = root.join("locked");
        fake_repository(&locked.join(".git"), "ref: refs/heads/main\n");
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();

        let detected = detect(&locked.join("inside"));

        // The administrator can read anything; for anyone else the
        // repository is invisible and the folder stands for itself.
        if fs::read_dir(&locked).is_err() {
            assert_eq!(detected.project.kind, ProjectKind::Folder);
            assert_eq!(detected.git, None);
        }
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn verbatim_drive_paths_are_simplified() {
        assert_eq!(
            simplify_verbatim(PathBuf::from(r"\\?\C:\Users\x")),
            PathBuf::from(r"C:\Users\x")
        );
        assert_eq!(
            simplify_verbatim(PathBuf::from(r"\\?\UNC\server\share")),
            PathBuf::from(r"\\?\UNC\server\share")
        );
    }
}
