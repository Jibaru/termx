//! Project detection against real Git repositories, made with `git` in a
//! temporary directory, and checked against what `git` itself says.

// The helpers below are test code too: a failed `unwrap` is a failed test.
#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::process::Command;

use termx_core::workspace::project::{ProjectKind, detect, normalize};

/// A temporary directory with its own, empty Git configuration.
struct Sandbox {
    dir: tempfile::TempDir,
}

impl Sandbox {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().expect("cannot create a temporary directory"),
        }
    }

    /// The directory, in the form detection reports paths in.
    fn root(&self) -> PathBuf {
        normalize(self.dir.path())
    }

    /// Runs `git` in `dir` and returns what it printed. Nothing of the
    /// configuration of the machine or of the user applies.
    fn git(&self, dir: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .current_dir(dir)
            .args([
                "-c",
                "user.name=termx tests",
                "-c",
                "user.email=tests@termx.invalid",
                "-c",
                "init.defaultBranch=main",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "core.autocrlf=false",
                "-c",
                "protocol.file.allow=always",
            ])
            .args(args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env(
                "GIT_CONFIG_GLOBAL",
                self.dir.path().join("no-such-gitconfig"),
            )
            .env("HOME", self.dir.path())
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .output()
            .expect("cannot run git");
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    /// Creates a repository with one commit on `main`.
    fn repository(&self, name: &str) -> PathBuf {
        let dir = name
            .split('/')
            .fold(self.root(), |dir, part| dir.join(part));
        std::fs::create_dir_all(&dir).unwrap();
        self.git(&dir, &["init", "--quiet"]);
        self.commit(&dir, "first");
        dir
    }

    fn commit(&self, dir: &Path, message: &str) {
        std::fs::write(dir.join(format!("{message}.txt")), message).unwrap();
        self.git(dir, &["add", "--all"]);
        self.git(dir, &["commit", "--quiet", "--message", message]);
    }

    /// What `git` says the top level of the working tree of `dir` is.
    fn top_level(&self, dir: &Path) -> PathBuf {
        normalize(Path::new(&self.git(dir, &["rev-parse", "--show-toplevel"])))
    }
}

#[test]
fn a_repository_is_found_from_any_folder_inside_it() {
    let sandbox = Sandbox::new();
    let repository = sandbox.repository("app");
    let nested = repository.join("src").join("deep").join("er");
    std::fs::create_dir_all(&nested).unwrap();

    for folder in [&repository, &nested] {
        let detected = detect(folder);

        assert_eq!(detected.project.kind, ProjectKind::Git);
        assert_eq!(detected.project.root, sandbox.top_level(folder));
        assert_eq!(detected.project.root, repository);
        assert_eq!(detected.project.name, "app");
        assert_eq!(detected.project.key, repository.to_string_lossy());
        let git = detected.git.expect("no git information");
        assert_eq!(git.branch.as_deref(), Some("main"));
        assert_eq!(
            git.branch.unwrap(),
            sandbox.git(folder, &["rev-parse", "--abbrev-ref", "HEAD"])
        );
        assert_eq!(git.commit, None);
        assert_eq!(git.worktree, None);
    }
}

#[test]
fn a_branch_with_slashes_and_a_switch_of_branch_are_read() {
    let sandbox = Sandbox::new();
    let repository = sandbox.repository("app");

    sandbox.git(
        &repository,
        &["checkout", "--quiet", "-b", "feature/login-form"],
    );
    assert_eq!(
        detect(&repository).git.unwrap().branch.as_deref(),
        Some("feature/login-form")
    );

    sandbox.git(&repository, &["checkout", "--quiet", "main"]);
    assert_eq!(
        detect(&repository).git.unwrap().branch.as_deref(),
        Some("main")
    );
}

#[test]
fn a_repository_without_a_commit_has_its_future_branch() {
    let sandbox = Sandbox::new();
    let dir = sandbox.root().join("empty");
    std::fs::create_dir_all(&dir).unwrap();
    sandbox.git(&dir, &["init", "--quiet"]);

    let detected = detect(&dir);

    assert_eq!(detected.project.kind, ProjectKind::Git);
    assert_eq!(detected.git.unwrap().branch.as_deref(), Some("main"));
}

#[test]
fn a_detached_head_gives_the_abbreviated_commit() {
    let sandbox = Sandbox::new();
    let repository = sandbox.repository("app");
    sandbox.commit(&repository, "second");
    sandbox.git(&repository, &["checkout", "--quiet", "--detach", "HEAD~1"]);
    let commit = sandbox.git(&repository, &["rev-parse", "HEAD"]);

    let git = detect(&repository).git.unwrap();

    assert_eq!(git.branch, None);
    assert_eq!(git.commit.as_deref(), Some(&commit[..7]));
}

#[test]
fn a_linked_worktree_belongs_to_its_repository() {
    let sandbox = Sandbox::new();
    let repository = sandbox.repository("app");
    let worktree = sandbox.root().join("app-review");
    sandbox.git(
        &repository,
        &[
            "worktree",
            "add",
            "--quiet",
            "-b",
            "review",
            &worktree.to_string_lossy(),
        ],
    );
    let inside = worktree.join("sub");
    std::fs::create_dir_all(&inside).unwrap();

    for folder in [&worktree, &inside] {
        let detected = detect(folder);

        // Grouped with the main repository...
        assert_eq!(detected.project.root, repository);
        assert_eq!(detected.project.name, "app");
        assert_eq!(detected.project.key, detect(&repository).project.key);
        // ...with its own branch, and named as a worktree.
        let git = detected.git.unwrap();
        assert_eq!(git.branch.as_deref(), Some("review"));
        assert_eq!(git.worktree.as_deref(), Some("app-review"));
        assert_eq!(sandbox.top_level(folder), worktree);
    }
    // The main working tree is not a worktree in that sense.
    let main = detect(&repository).git.unwrap();
    assert_eq!(main.branch.as_deref(), Some("main"));
    assert_eq!(main.worktree, None);
}

#[test]
fn a_detached_worktree_gives_its_commit() {
    let sandbox = Sandbox::new();
    let repository = sandbox.repository("app");
    let worktree = sandbox.root().join("detached");
    sandbox.git(
        &repository,
        &[
            "worktree",
            "add",
            "--quiet",
            "--detach",
            &worktree.to_string_lossy(),
        ],
    );
    let commit = sandbox.git(&worktree, &["rev-parse", "HEAD"]);

    let git = detect(&worktree).git.unwrap();

    assert_eq!(git.branch, None);
    assert_eq!(git.commit.as_deref(), Some(&commit[..7]));
    assert_eq!(git.worktree.as_deref(), Some("detached"));
}

#[test]
fn a_repository_nested_in_another_is_its_own_project() {
    let sandbox = Sandbox::new();
    let outer = sandbox.repository("outer");
    let inner = sandbox.repository("outer/vendor/inner");
    sandbox.git(&inner, &["checkout", "--quiet", "-b", "inner-branch"]);
    let inside_inner = inner.join("src");
    std::fs::create_dir_all(&inside_inner).unwrap();
    let beside_inner = outer.join("vendor");

    let detected = detect(&inside_inner);
    assert_eq!(detected.project.root, inner);
    assert_eq!(detected.project.root, sandbox.top_level(&inside_inner));
    assert_eq!(detected.project.name, "inner");
    assert_eq!(
        detected.git.unwrap().branch.as_deref(),
        Some("inner-branch")
    );

    let detected = detect(&beside_inner);
    assert_eq!(detected.project.root, outer);
    assert_eq!(detected.git.unwrap().branch.as_deref(), Some("main"));
}

#[test]
fn a_submodule_is_its_own_project() {
    let sandbox = Sandbox::new();
    let library = sandbox.repository("library");
    let app = sandbox.repository("app");
    sandbox.git(
        &app,
        &[
            "submodule",
            "--quiet",
            "add",
            &library.to_string_lossy(),
            "deps/library",
        ],
    );
    let submodule = app.join("deps").join("library");
    // A submodule keeps its Git directory in the parent: `.git` is a file.
    assert!(submodule.join(".git").is_file());

    let detected = detect(&submodule);

    assert_eq!(detected.project.kind, ProjectKind::Git);
    assert_eq!(detected.project.root, submodule);
    assert_eq!(detected.project.root, sandbox.top_level(&submodule));
    assert_eq!(detected.project.name, "library");
    let git = detected.git.unwrap();
    assert_eq!(git.worktree, None);
    assert!(
        git.branch.is_some() || git.commit.is_some(),
        "a submodule has a branch or a commit"
    );
    // The parent is still found from its own folders.
    assert_eq!(detect(&app.join("deps")).project.root, app);
}

#[test]
fn a_bare_repository_is_a_project_and_so_are_its_worktrees() {
    let sandbox = Sandbox::new();
    let source = sandbox.repository("source");
    let bare = sandbox.root().join("central.git");
    sandbox.git(
        &sandbox.root(),
        &[
            "clone",
            "--quiet",
            "--bare",
            &source.to_string_lossy(),
            &bare.to_string_lossy(),
        ],
    );

    let detected = detect(&bare);
    assert_eq!(detected.project.kind, ProjectKind::Git);
    assert_eq!(detected.project.root, bare);
    assert_eq!(detected.project.name, "central");
    assert_eq!(detected.git.unwrap().branch.as_deref(), Some("main"));
    // From inside the repository too.
    assert_eq!(detect(&bare.join("refs")).project.root, bare);

    let worktree = sandbox.root().join("checkout");
    sandbox.git(
        &bare,
        &[
            "worktree",
            "add",
            "--quiet",
            "-b",
            "work",
            &worktree.to_string_lossy(),
        ],
    );
    let detected = detect(&worktree);
    assert_eq!(detected.project.root, bare);
    assert_eq!(detected.project.name, "central");
    let git = detected.git.unwrap();
    assert_eq!(git.branch.as_deref(), Some("work"));
    assert_eq!(git.worktree.as_deref(), Some("checkout"));
}

#[test]
fn a_folder_inside_dot_git_belongs_to_the_repository() {
    let sandbox = Sandbox::new();
    let repository = sandbox.repository("app");

    for folder in [
        repository.join(".git"),
        repository.join(".git").join("refs"),
    ] {
        let detected = detect(&folder);

        assert_eq!(detected.project.root, repository, "{folder:?}");
        assert_eq!(detected.project.name, "app");
        assert_eq!(detected.git.unwrap().branch.as_deref(), Some("main"));
    }
}

#[test]
fn a_folder_outside_any_repository_is_a_plain_folder() {
    let sandbox = Sandbox::new();
    let folder = sandbox.root().join("notes").join("2026");
    std::fs::create_dir_all(&folder).unwrap();
    // A sibling repository does not make it one.
    sandbox.repository("app");

    let detected = detect(&folder);

    // Unless the temporary directory itself is inside a repository, which
    // would be the machine's doing, not ours.
    if detect(&sandbox.root()).project.kind == ProjectKind::Folder {
        assert_eq!(detected.project.kind, ProjectKind::Folder);
        assert_eq!(detected.project.root, folder);
        assert_eq!(detected.project.name, "2026");
        assert_eq!(detected.git, None);
    }
}

#[test]
fn a_folder_reached_through_a_symbolic_link_is_the_same_project() {
    let sandbox = Sandbox::new();
    let repository = sandbox.repository("app");
    let link = sandbox.root().join("link-to-app");
    #[cfg(unix)]
    let linked = std::os::unix::fs::symlink(&repository, &link);
    #[cfg(windows)]
    let linked = std::os::windows::fs::symlink_dir(&repository, &link);
    // Creating links needs a privilege on Windows that a machine may lack.
    if linked.is_err() {
        return;
    }

    assert_eq!(detect(&link).project, detect(&repository).project);
}
