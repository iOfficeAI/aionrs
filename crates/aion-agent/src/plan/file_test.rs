use super::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_joins_session_id() {
        let dir = Path::new("/tmp/plans");
        let path = plan_file_path(dir, "session-abc");
        assert_eq!(path, PathBuf::from("/tmp/plans/session-abc.md"));
    }

    #[test]
    fn path_handles_complex_session_id() {
        let dir = Path::new("/data");
        let path = plan_file_path(dir, "2024-01-01_abc123");
        assert_eq!(path, PathBuf::from("/data/2024-01-01_abc123.md"));
    }

    #[test]
    fn write_creates_parent_dirs() {
        let tmp = tempfile::TempDir::new().unwrap();
        let nested = tmp.path().join("a").join("b").join("plan.md");

        write_plan(&nested, "# Plan").unwrap();

        assert!(nested.exists());
        assert_eq!(fs::read_to_string(&nested).unwrap(), "# Plan");
    }

    #[test]
    fn write_overwrites_existing() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("plan.md");

        write_plan(&path, "v1").unwrap();
        write_plan(&path, "v2").unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), "v2");
    }

    #[test]
    fn read_existing_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("plan.md");
        fs::write(&path, "# My Plan\nStep 1").unwrap();

        let result = read_plan(&path).unwrap();
        assert_eq!(result, Some("# My Plan\nStep 1".to_string()));
    }

    #[test]
    fn read_nonexistent_returns_none() {
        let path = Path::new("/nonexistent/path/plan.md");
        let result = read_plan(path).unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn write_then_read_roundtrip() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("plans").join("sess.md");

        let content = "# Implementation Plan\n\n## Context\nRefactor auth module";
        write_plan(&path, content).unwrap();

        let read_back = read_plan(&path).unwrap();
        assert_eq!(read_back, Some(content.to_string()));
    }
}
