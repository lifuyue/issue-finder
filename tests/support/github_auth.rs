use std::ffi::OsString;

/// Clear inherited GitHub credentials while holding the test environment lock.
pub struct GitHubAuthGuard {
    previous: [Option<OsString>; 2],
}

impl GitHubAuthGuard {
    pub fn clear() -> Self {
        let previous = [
            std::env::var_os("GH_TOKEN"),
            std::env::var_os("GITHUB_TOKEN"),
        ];
        std::env::remove_var("GH_TOKEN");
        std::env::remove_var("GITHUB_TOKEN");
        Self { previous }
    }
}

impl Drop for GitHubAuthGuard {
    fn drop(&mut self) {
        for (key, previous) in ["GH_TOKEN", "GITHUB_TOKEN"].into_iter().zip(&self.previous) {
            if let Some(value) = previous {
                std::env::set_var(key, value);
            } else {
                std::env::remove_var(key);
            }
        }
    }
}
