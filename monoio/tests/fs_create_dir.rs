#![cfg(feature = "mkdirat")]

use monoio::fs;
use tempfile::tempdir;

#[monoio::test_all]
async fn create_single_dirctory() {
    let temp_dir = tempdir().unwrap();
    let path = temp_dir.path().join("test");

    fs::create_dir(&path).await.unwrap();

    assert!(path.exists());

    std::fs::remove_dir(&path).unwrap();

    assert!(!path.exists());

    fs::create_dir_all(&path).await.unwrap();

    assert!(path.exists());
}

#[monoio::test_all]
async fn create_nested_directories() {
    let temp_dir = tempdir().unwrap();
    let path = temp_dir.path().join("test/foo/bar");

    fs::create_dir_all(&path).await.unwrap();

    assert!(path.exists());
}

#[monoio::test_all]
async fn create_existing_directory() {
    let temp_dir = tempdir().unwrap();

    fs::create_dir_all(temp_dir.path()).await.unwrap();
}

#[monoio::test_all]
async fn create_invalid_path() {
    let temp_dir = tempdir().unwrap();

    let mut path = temp_dir.path().display().to_string();
    path += "invalid_dir/\0";

    let res = fs::create_dir_all(path).await;

    assert!(res.is_err());
}

#[monoio::test_all]
async fn create_directory_with_special_characters() {
    let temp_dir = tempdir().unwrap();

    let path = temp_dir.path().join("foo/😀");

    fs::create_dir_all(&path).await.unwrap();

    assert!(path.exists());
}

#[monoio::test_all]
async fn create_directory_where_file_exists() {
    let temp_file = tempfile::NamedTempFile::new().unwrap();
    fs::write(temp_file.path(), "foo bar").await.0.unwrap();

    let res = fs::create_dir(temp_file.path()).await;

    assert!(res.is_err());

    let res = fs::create_dir_all(temp_file.path()).await;

    assert!(res.is_err());
}

#[monoio::test_all]
async fn create_directory_with_symlink() {
    let temp_dir = tempdir().unwrap();

    let target = temp_dir.path().join("foo");

    fs::create_dir_all(&target).await.unwrap();

    let link = temp_dir.path().join("bar");
    let to_create = link.join("nested");

    #[cfg(unix)]
    std::os::unix::fs::symlink(&target, &link).unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(&target, &link).unwrap();

    fs::create_dir_all(&to_create).await.unwrap();

    assert!(to_create.exists());
    assert!(target.join("nested").exists());
}

#[cfg(unix)]
#[monoio::test_all]
async fn create_very_long_path() {
    let temp_dir = tempdir().unwrap();

    let mut path = temp_dir.path().to_path_buf();
    for _ in 0..255 {
        path.push("a/");
    }

    fs::create_dir_all(&path).await.unwrap();

    assert!(path.exists());
}

#[monoio::test_all]
async fn create_dir_all_concurrent_race() {
    // Test that concurrent create_dir_all calls to the same path don't fail
    // with AlreadyExists errors. This is a regression test for the race
    // condition where two tasks creating the same nested directory could
    // interleave and cause one to fail.
    let temp_dir = tempdir().unwrap();
    let path = temp_dir.path().join("a/b/c/d/e");

    let p1 = path.clone();
    let p2 = path.clone();

    let t1 = monoio::spawn(async move { fs::create_dir_all(&p1).await });
    let t2 = monoio::spawn(async move { fs::create_dir_all(&p2).await });

    let r1 = t1.await;
    let r2 = t2.await;

    assert!(r1.is_ok(), "first task failed: {:?}", r1);
    assert!(r2.is_ok(), "second task failed: {:?}", r2);
    assert!(path.exists());
}

#[cfg(unix)]
#[monoio::test_all]
async fn create_directory_with_permission_issue() {
    use std::os::unix::fs::PermissionsExt;

    let temp_dir = tempdir().unwrap();

    let target = temp_dir.path().join("foo");

    fs::create_dir_all(&target).await.unwrap();

    // use `std`'s due to the `monoio`'s `set_permissions` is not implement.
    let mut perm = std::fs::metadata(&target).unwrap().permissions();
    perm.set_mode(0o400);

    std::fs::set_permissions(&target, perm.clone()).unwrap();

    let path = target.join("bar");
    let res = fs::create_dir_all(&path).await;
    assert!(res.is_err());

    perm.set_mode(0o700);
    std::fs::set_permissions(&target, perm).unwrap();

    fs::create_dir_all(&path).await.unwrap();

    assert!(path.exists());
}
