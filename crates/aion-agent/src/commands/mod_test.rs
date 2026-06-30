use super::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_find_by_name() {
        let registry = default_registry();
        assert!(registry.find("compact").is_some());
        assert!(registry.find("clear").is_some());
        assert!(registry.find("help").is_some());
        assert!(registry.find("quit").is_some());
    }

    #[test]
    fn registry_find_by_alias() {
        let registry = default_registry();
        assert!(registry.find("exit").is_some());
        let cmd = registry.find("exit").unwrap();
        assert_eq!(cmd.name(), "quit");
    }

    #[test]
    fn registry_find_unknown_returns_none() {
        let registry = default_registry();
        assert!(registry.find("nonexistent").is_none());
    }

    #[test]
    fn registry_all_returns_all_commands() {
        let registry = default_registry();
        assert_eq!(registry.all().len(), 4);
    }
}
