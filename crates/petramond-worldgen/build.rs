use std::path::{Path, PathBuf};

const SOURCES: [&str; 3] = ["src", "../petramond-math/src", "../petramond-world/src"];

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("reading a stamped source dir") {
        let path = entry.expect("reading a stamped source entry").path();
        if path.is_dir() {
            collect(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

fn main() {
    let mut files = Vec::new();
    for dir in SOURCES {
        println!("cargo:rerun-if-changed={dir}");
        collect(Path::new(dir), &mut files);
    }
    files.sort();
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for file in &files {
        let name = file.to_string_lossy().replace('\\', "/");
        let text = std::fs::read_to_string(file).expect("reading a stamped source file");
        for b in name
            .bytes()
            .chain([0])
            .chain(text.replace("\r\n", "\n").bytes())
        {
            hash ^= u64::from(b);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    std::fs::write(
        out.join("engine_stamp.rs"),
        format!("pub const ENGINE_STAMP: u64 = {hash:#x};\n"),
    )
    .expect("writing the engine stamp");
}
