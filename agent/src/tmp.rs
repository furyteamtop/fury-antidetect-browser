//! A temporary directory that removes itself.
//!
//! Tests here write real files — profile directories, databases, export
//! archives — because the code under test does. Left behind, they accumulate:
//! a few hundred directories in the machine's temp folder after a week of
//! runs, each holding a file called `Cookies`. Harmless, and unsettling to
//! find. So every test directory is owned by a value, and the value cleans up.

pub struct TempDir(std::path::PathBuf);

impl TempDir {
    /// A fresh directory tagged with what it is for, so a run interrupted
    /// before `Drop` leaves something identifiable rather than a bare uuid.
    pub fn new(tag: &str) -> Self {
        let d = std::env::temp_dir().join(format!("fury-{tag}-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&d).unwrap();
        Self(d)
    }
}

impl std::ops::Deref for TempDir {
    type Target = std::path::Path;
    fn deref(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        // A failing test should not also fail to clean up, and a cleanup that
        // panics would hide the assertion that actually failed.
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
