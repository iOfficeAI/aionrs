use super::*;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::register_hello_skill;
    use crate::bundled::{clear_bundled_skills, get_bundled_skills};
    use serial_test::serial;

    // TC-10.18: hello skill fields are correct
    #[test]
    #[serial]
    fn tc_10_18_hello_skill_fields_correct() {
        clear_bundled_skills();
        register_hello_skill();
        let skills = get_bundled_skills();
        let hello = skills
            .iter()
            .find(|s| s.name == "hello")
            .expect("hello skill should be registered");
        assert!(hello.user_invocable, "hello should be user_invocable");
        assert!(
            !hello.description.is_empty(),
            "hello should have a non-empty description"
        );
    }
}
