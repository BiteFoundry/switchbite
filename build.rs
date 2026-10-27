use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=.git");
    if let Ok(out)=Command::new("git").args(["rev-parse","--git-path","HEAD"]).output() && out.status.success() && let Ok(path)=String::from_utf8(out.stdout) { println!("cargo:rerun-if-changed={}",path.trim()); }
    if let Ok(out)=Command::new("git").args(["symbolic-ref","HEAD"]).output() && out.status.success() && let Ok(reference)=String::from_utf8(out.stdout)
        && let Ok(out)=Command::new("git").args(["rev-parse","--git-path",reference.trim()]).output() && out.status.success() && let Ok(path)=String::from_utf8(out.stdout) { println!("cargo:rerun-if-changed={}",path.trim()); }
    println!("cargo:rerun-if-env-changed=SWITCHBITE_BUILD_COMMIT");
    let commit = std::env::var("SWITCHBITE_BUILD_COMMIT")
        .ok()
        .or_else(|| {
            Command::new("git")
                .args(["rev-parse", "HEAD"])
                .output()
                .ok()
                .filter(|o| o.status.success())
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .map(|s| s.trim().to_owned())
        })
        .unwrap_or_else(|| "unknown".into());
    if commit.bytes().all(|b| b.is_ascii_hexdigit()) && commit.len() == 40 {
        println!("cargo:rustc-env=SWITCHBITE_BUILD_COMMIT={commit}");
    }
}
