use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;

/// Resolve the cache directory, falling back through XDG → home → /tmp.
///
/// The native build normally hits `dirs_next::cache_dir()` first. The WASI
/// sandbox host sets `HOME=/` but no `XDG_CACHE_HOME`, so `cache_dir()`
/// returns `None` and we land on `~/.cache/outside`. `/tmp` exists too
/// (auto-provisioned by the host).
fn cache_root() -> PathBuf {
    dirs_next::cache_dir()
        .or_else(|| std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from))
        .or_else(|| dirs_next::home_dir().map(|h| h.join(".cache")))
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(env!("CARGO_PKG_NAME"))
}

/// Generates a cache file path for the given data type and content.
///
/// Returns the full path to the cache file as a string. If the cache
/// directory cannot be created, prints a warning and returns the path
/// anyway — savefile read/write will fail gracefully and the caller will
/// fall back to a live fetch.
pub fn get_cached_file(datatype: &str, content: &str) -> String {
    let mut hasher = DefaultHasher::new();
    content.hash(&mut hasher);
    let hash = format!("{:x}", hasher.finish());

    let root = cache_root();
    if let Err(e) = std::fs::create_dir_all(&root) {
        eprintln!(
            "warning: cache dir {} unavailable ({}), caching disabled",
            root.display(),
            e
        );
    }

    root.join(format!("{datatype}-{hash}.cache")).display().to_string()
}
