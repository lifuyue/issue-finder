use std::fs::{self, OpenOptions};
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

pub struct EnvLock {
    path: PathBuf,
}

impl EnvLock {
    pub fn acquire() -> Self {
        let path = std::env::temp_dir().join("issue-finder-test-env.lock");
        let started = Instant::now();

        loop {
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(_) => return Self { path },
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    if stale_lock(&path) {
                        let _ = fs::remove_file(&path);
                    } else if started.elapsed() > Duration::from_secs(30) {
                        panic!("timed out waiting for {}", path.display());
                    }
                    thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("unable to create {}: {error}", path.display()),
            }
        }
    }
}

impl Drop for EnvLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn stale_lock(path: &PathBuf) -> bool {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.elapsed().ok())
        .is_some_and(|age| age > Duration::from_secs(120))
}
