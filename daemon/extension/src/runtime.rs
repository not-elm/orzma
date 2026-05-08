// daemon/extension/src/runtime.rs
use std::{fs, path::{Path, PathBuf}};

pub struct RuntimeRoot {
    root: PathBuf,
    bin_dir: PathBuf,
    sock_dir: PathBuf,
}

impl RuntimeRoot {
    pub fn new_in(parent: &Path, pid: u32) -> std::io::Result<Self> {
        let root = parent.join(pid.to_string());
        let bin_dir = root.join("bin");
        let sock_dir = root.join("sock");
        fs::create_dir_all(&root)?;
        fs::create_dir_all(&bin_dir)?;
        fs::create_dir_all(&sock_dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for p in [&root, &bin_dir, &sock_dir] {
                fs::set_permissions(p, fs::Permissions::from_mode(0o700))?;
            }
        }
        Ok(Self { root, bin_dir, sock_dir })
    }

    pub fn root(&self) -> &Path { &self.root }
    pub fn bin_dir(&self) -> &Path { &self.bin_dir }
    pub fn sock_dir(&self) -> &Path { &self.sock_dir }
}

impl Drop for RuntimeRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use tempfile::tempdir;

    #[test]
    fn new_in_creates_subdirs_with_0700() {
        let parent = tempdir().unwrap();
        let rt = RuntimeRoot::new_in(parent.path(), 12345).unwrap();
        for p in [rt.root(), rt.bin_dir(), rt.sock_dir()] {
            let mode = fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o700, "wrong mode on {:?}", p);
        }
    }

    #[test]
    fn drop_removes_tree() {
        let parent = tempdir().unwrap();
        let path = {
            let rt = RuntimeRoot::new_in(parent.path(), 99999).unwrap();
            rt.root().to_path_buf()
        };
        assert!(!path.exists(), "tree should be removed by Drop");
    }
}
