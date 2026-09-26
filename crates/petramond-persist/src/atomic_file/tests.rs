use super::*;

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("petramond-atomic-{}-{tag}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn names(dir: &Path) -> Vec<String> {
    let mut names: Vec<_> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn a_failed_replace_keeps_the_previous_file_and_leaves_no_temporary() {
    let dir = temp_dir("replace");
    let path = dir.join("a.dat");
    replace(&path, b"one").unwrap();
    let failed = replace_with(&path, Durability::Synced, |file| {
        file.write_all(b"partial")?;
        Err(io::Error::other("disk gone"))
    });
    assert!(failed.is_err());
    assert_eq!(fs::read(&path).unwrap(), b"one");
    assert_eq!(names(&dir), ["a.dat"]);
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn publishing_never_overwrites() {
    let dir = temp_dir("publish");
    let path = dir.join("a.dat");
    publish_new(&path, |file| file.write_all(b"first")).unwrap();
    let second = publish_new(&path, |file| file.write_all(b"second"));
    assert_eq!(second.unwrap_err().kind(), io::ErrorKind::AlreadyExists);
    assert_eq!(fs::read(&path).unwrap(), b"first");
    assert_eq!(names(&dir), ["a.dat"]);
    fs::remove_dir_all(&dir).unwrap();
}
