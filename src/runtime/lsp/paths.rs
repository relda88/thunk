use std::path::{Path, PathBuf};

pub(super) fn path_to_file_uri(path: &Path) -> String {
    let path = path.to_string_lossy();
    let escaped = path
        .replace('%', "%25")
        .replace(' ', "%20")
        .replace('#', "%23")
        .replace('?', "%3F");
    format!("file://{escaped}")
}

pub(super) fn file_uri_to_path(uri: &str) -> Option<PathBuf> {
    let path = uri.strip_prefix("file://")?;
    let decoded = path
        .replace("%20", " ")
        .replace("%23", "#")
        .replace("%3F", "?")
        .replace("%25", "%");
    Some(PathBuf::from(decoded))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn builds_file_uri() {
        let uri = path_to_file_uri(Path::new("/tmp/hello world.rs"));
        assert_eq!(uri, "file:///tmp/hello%20world.rs");
    }

    #[test]
    fn round_trips_plain_path() {
        let original = Path::new("/home/user/project/src/main.rs");
        let uri = path_to_file_uri(original);
        let recovered = file_uri_to_path(&uri).expect("round trip");
        assert_eq!(recovered, original);
    }
}
