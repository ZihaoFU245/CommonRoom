use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8(output.stdout).ok()?;
    Some(value.trim().to_owned())
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    // Follow both symbolic and detached HEAD, including linked worktrees and
    // packed refs, so a new commit refreshes an otherwise unchanged binary.
    for name in [
        Some("HEAD".to_owned()),
        git(&["symbolic-ref", "-q", "HEAD"]),
        Some("packed-refs".to_owned()),
    ]
    .into_iter()
    .flatten()
    {
        if let Some(path) = git(&["rev-parse", "--git-path", &name]) {
            println!("cargo:rerun-if-changed={path}");
        }
    }
    let commit = git(&["rev-parse", "--verify", "HEAD"]).unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=CHAT_COMMIT={commit}");
}
