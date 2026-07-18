pub(crate) fn crash_at(boundary: &str) {
    if std::env::var("ISSUE_FINDER_EVAL_CRASH_AT").ok().as_deref() == Some(boundary) {
        eprintln!("Issue Finder injected evaluation crash at {boundary}");
        std::process::abort();
    }
}
