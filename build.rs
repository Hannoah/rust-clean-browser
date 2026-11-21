use std::env;
use std::fs;
use std::path::{Path, PathBuf};

/// Every file under `dir` as (path relative to `root` with `/`, absolute path).
fn collect(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let p = e.path();
        if p.is_dir() {
            collect(root, &p, out);
        } else if let Ok(rel) = p.strip_prefix(root) {
            out.push((rel.to_string_lossy().replace('\\', "/"), p.clone()));
        }
    }
}

/// Embeds the pdf.js annotator (`vendor/pdfjs`), deflate-compressed, as `FILES` for `src/pdfjs.rs`.
fn embed_pdfjs(out_path: &Path) {
    println!("cargo:rerun-if-changed=vendor/pdfjs");
    let root = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap_or_default())
        .join("vendor")
        .join("pdfjs");
    let mut files = Vec::new();
    collect(&root, &root, &mut files);
    // LICENSE ships with the source tree, not in the served app.
    files.retain(|(rel, _)| rel != "LICENSE");
    let mut code = String::from("pub static FILES: &[(&str, &[u8])] = &[\n");
    for (i, (rel, path)) in files.iter().enumerate() {
        let Ok(data) = fs::read(path) else { continue };
        let z = out_path.join(format!("pdfjs_{i}.z"));
        if fs::write(&z, miniz_oxide::deflate::compress_to_vec(&data, 9)).is_ok() {
            code.push_str(&format!(
                "    ({rel:?}, include_bytes!({:?})),\n",
                z.display().to_string()
            ));
        }
    }
    code.push_str("];\n");
    let _ = fs::write(out_path.join("pdfjs_assets.rs"), code);
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    let Ok(out_dir) = env::var("OUT_DIR") else {
        return;
    };
    let out_path = PathBuf::from(&out_dir);
    embed_pdfjs(&out_path);

    // OUT_DIR looks like: target/<profile>/build/<pkg>-<hash>/out
    let Some(profile_dir) = out_path.ancestors().nth(3).map(Path::to_path_buf) else {
        return;
    };
    let build_dir = profile_dir.join("build");

    let arch_dir = match env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("x86_64") => "x64",
        Ok("x86") => "x86",
        Ok("aarch64") => "arm64",
        _ => "x64",
    };

    // 1) Copy WebView2Loader.dll from webview2-com-sys's build output into target/<profile>/
    if let Ok(entries) = fs::read_dir(&build_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with("webview2-com-sys-") {
                let candidate = entry.path().join("out").join(arch_dir).join("WebView2Loader.dll");
                if candidate.is_file() {
                    let _ = fs::copy(&candidate, profile_dir.join("WebView2Loader.dll"));
                    break;
                }
            }
        }
    }

    // 2) Embed the app icon into the .exe (Explorer / taskbar / shortcuts / installer).
    // embed-resource picks windres (GNU) or rc.exe/llvm-rc (MSVC) automatically.
    println!("cargo:rerun-if-changed=assets/icon.ico");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap_or_default());
        let ico = manifest_dir.join("assets").join("icon.ico");
        let rc = out_path.join("app.rc");
        let rc_src = format!("1 ICON \"{}\"\n", ico.display().to_string().replace('\\', "/"));
        if ico.is_file()
            && fs::write(&rc, rc_src).is_ok()
            && let Err(e) = embed_resource::compile(&rc, embed_resource::NONE).manifest_optional()
        {
            println!("cargo:warning=exe icon not embedded: {e}");
        }
    }

    // 3) On windows-gnullvm, copy libunwind.dll from PATH next to the binary so
    // launching from Windows Explorer works without llvm-mingw on global PATH.
    if env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("gnu")
        && let Some(path_var) = env::var_os("PATH")
    {
        for dir in env::split_paths(&path_var) {
            let candidate = dir.join("libunwind.dll");
            if candidate.is_file() {
                let _ = fs::copy(&candidate, profile_dir.join("libunwind.dll"));
                break;
            }
        }
    }
}
