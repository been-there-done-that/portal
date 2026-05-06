/// Returns true if cwd is inside a git linked worktree (not the main worktree).
/// A linked worktree has `.git` as a FILE, not a directory.
pub fn is_linked_worktree(cwd: &std::path::Path) -> bool {
    let mut dir = cwd;
    loop {
        let git_path = dir.join(".git");
        if git_path.is_file() {
            return true;
        }
        if git_path.is_dir() {
            return false;
        }
        match dir.parent() {
            Some(p) => dir = p,
            None => return false,
        }
    }
}

/// Returns the current branch name for a linked worktree, or None for detached HEAD.
pub fn current_branch(cwd: &std::path::Path) -> Option<String> {
    let mut dir = cwd;
    let git_file = loop {
        let git_path = dir.join(".git");
        if git_path.is_file() {
            break git_path;
        }
        match dir.parent() {
            Some(p) => dir = p,
            None => return None,
        }
    };
    let contents = std::fs::read_to_string(&git_file).ok()?;
    let git_dir = contents
        .strip_prefix("gitdir: ")?
        .trim()
        .to_string();
    let git_dir = std::path::Path::new(&git_dir);
    let head = std::fs::read_to_string(git_dir.join("HEAD")).ok()?;
    let branch = head
        .trim()
        .strip_prefix("ref: refs/heads/")?
        .to_string();
    Some(sanitize_branch(&branch))
}

pub fn sanitize_branch(name: &str) -> String {
    let lower = name.to_lowercase();
    let replaced: String = lower
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '-' })
        .collect();
    let mut result = String::new();
    let mut last_hyphen = false;
    for c in replaced.chars() {
        if c == '-' {
            if !last_hyphen {
                result.push(c);
            }
            last_hyphen = true;
        } else {
            result.push(c);
            last_hyphen = false;
        }
    }
    let trimmed = result.trim_matches('-');
    trimmed.chars().take(40).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn main_worktree_is_not_linked() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        fs::create_dir(root.join(".git")).unwrap();
        assert!(!is_linked_worktree(root));
    }

    #[test]
    fn linked_worktree_detected() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        fs::write(
            root.join(".git"),
            "gitdir: /some/main/.git/worktrees/feature\n",
        )
        .unwrap();
        assert!(is_linked_worktree(root));
    }

    #[test]
    fn no_git_returns_false() {
        let tmp = TempDir::new().unwrap();
        assert!(!is_linked_worktree(tmp.path()));
    }

    #[test]
    fn current_branch_from_linked_worktree() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let fake_git_dir = tmp.path().join("fake_git");
        fs::create_dir_all(&fake_git_dir).unwrap();
        fs::write(
            fake_git_dir.join("HEAD"),
            "ref: refs/heads/fix/auth-bug\n",
        )
        .unwrap();
        fs::write(
            root.join(".git"),
            format!("gitdir: {}\n", fake_git_dir.display()),
        )
        .unwrap();
        let branch = current_branch(root);
        assert_eq!(branch.as_deref(), Some("fix-auth-bug"));
    }

    #[test]
    fn detached_head_returns_none() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let fake_git_dir = tmp.path().join("fake_git");
        fs::create_dir_all(&fake_git_dir).unwrap();
        fs::write(
            fake_git_dir.join("HEAD"),
            "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2\n",
        )
        .unwrap();
        fs::write(
            root.join(".git"),
            format!("gitdir: {}\n", fake_git_dir.display()),
        )
        .unwrap();
        assert!(current_branch(root).is_none());
    }

    #[test]
    fn sanitize_branch_slash() {
        assert_eq!(sanitize_branch("fix/foo"), "fix-foo");
    }

    #[test]
    fn sanitize_branch_uppercase() {
        assert_eq!(sanitize_branch("Feature/FOO"), "feature-foo");
    }

    #[test]
    fn sanitize_branch_consecutive_dashes() {
        assert_eq!(sanitize_branch("fix--bar"), "fix-bar");
    }

    #[test]
    fn sanitize_branch_truncation() {
        let long_name = "a".repeat(50);
        let result = sanitize_branch(&long_name);
        assert_eq!(result.len(), 40);
    }
}
