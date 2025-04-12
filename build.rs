use std::path::Path;

fn get_git_version() -> Result<String, Box<dyn std::error::Error>> {
    if Path::new(".git/HEAD").exists() {
        println!("cargo:rerun-if-changed=.git/HEAD");
        let r = std::fs::read_to_string(".git/HEAD").unwrap();
        let mut r = r.split_ascii_whitespace();
        r.next();
        let branch = r.next().unwrap();
        println!("cargo:rerun-if-changed=.git/{}", branch);
    }
    let git_version = std::process::Command::new("git")
        .arg("describe")
        .arg("--dirty")
        .output()?;
    let mut git_version = String::from_utf8(git_version.stdout)?;
    git_version.pop();
    let full_version = git_version.strip_prefix('v').ok_or("error")?.to_owned();
    Ok(full_version)
}
fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    let full_version =
        get_git_version().unwrap_or_else(|_| std::env::var("CARGO_PKG_VERSION").unwrap());

    assert!(
        full_version.starts_with(&std::env::var("CARGO_PKG_VERSION").unwrap()),
        "latest git tag does not match the version set in cargo: {} vs {}",
        full_version,
        std::env::var("CARGO_PKG_VERSION").unwrap()
    );

    println!("cargo:rustc-env=BUILD_FULL_VERSION={}", full_version);
}
