use std::path::{Path, PathBuf};

const ALLOWED: [&str; 2] = [
    "crates/petramond-world/src/block/shape_kind/",
    "crates/petramond-world/src/block/shape_kind.rs",
];

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if path.is_dir() {
            if name != "target" && name != "tests" {
                rust_files(&path, out);
            }
        } else if name.ends_with(".rs") && !name.ends_with("tests.rs") {
            out.push(path);
        }
    }
}

fn production(text: &str) -> &str {
    ["#[cfg(test)]\nmod ", "#[cfg(test)]\r\nmod "]
        .into_iter()
        .filter_map(|marker| text.find(marker))
        .min()
        .map_or(text, |end| &text[..end])
}

#[test]
fn production_stops_at_test_module_with_either_line_ending() {
    for newline in ["\n", "\r\n"] {
        let prefix = format!("fn production_code() {{}}{newline}");
        let source = format!("{prefix}#[cfg(test)]{newline}mod tests {{ ShapeFamily::Cube }}");
        assert_eq!(production(&source), prefix);
    }
}

#[test]
fn workspace_names_no_shape_family_outside_shape_kind() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut files = Vec::new();
    rust_files(&root.join("src"), &mut files);
    rust_files(&root.join("crates"), &mut files);
    assert!(
        files.len() > 100,
        "the scan must see the workspace (found {} files under {})",
        files.len(),
        root.display()
    );
    let mut offenders = Vec::new();
    for path in files {
        let rel = path
            .strip_prefix(&root)
            .expect("scanned under the root")
            .to_string_lossy()
            .replace('\\', "/");
        if ALLOWED.iter().any(|a| rel.starts_with(a)) {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("readable source");
        for (i, line) in production(&text).lines().enumerate() {
            if line.contains("ShapeFamily::") {
                offenders.push(format!("{rel}:{}: {}", i + 1, line.trim()));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "production code branches on a shape family outside block/shape_kind — ask a \
         facet instead:\n{}",
        offenders.join("\n")
    );
}
