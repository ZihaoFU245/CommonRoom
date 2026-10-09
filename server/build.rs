use std::{env, fs, path::Path};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let release = env::var("PROFILE").as_deref() == Ok("release");
    let root = if !release && Path::new("../web/dist-debug/index.html").exists() {
        Path::new("../web/dist-debug")
    } else {
        Path::new("../web/dist")
    };
    println!("cargo:rerun-if-changed=../web/dist");
    println!("cargo:rerun-if-changed=../web/dist-debug");
    let mut files = Vec::new();
    if root.exists() {
        collect(root, root, &mut files)?;
    }
    if release && !root.join("index.html").exists() {
        return Err(
            "Build the frontend first: pnpm --dir web install && pnpm --dir web build".into(),
        );
    }
    files.sort();
    let mut code = String::from("pub static ASSETS: &[(&str, &[u8])] = &[\n");
    for (url, path) in files {
        code.push_str(&format!("({url:?}, include_bytes!({path:?})),\n"));
    }
    code.push_str("];\n");
    fs::write(Path::new(&env::var("OUT_DIR")?).join("assets.rs"), code)?;
    Ok(())
}

fn collect(
    root: &Path,
    path: &Path,
    files: &mut Vec<(String, String)>,
) -> Result<(), Box<dyn std::error::Error>> {
    for entry in fs::read_dir(path)? {
        let path = entry?.path();
        if path.is_dir() {
            collect(root, &path, files)?;
        } else {
            files.push((
                format!(
                    "/{}",
                    path.strip_prefix(root)?
                        .to_string_lossy()
                        .replace('\\', "/")
                ),
                fs::canonicalize(path)?.to_string_lossy().into_owned(),
            ));
        }
    }
    Ok(())
}
