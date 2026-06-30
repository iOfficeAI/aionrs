use crate::types::SkillMetadata;

// Re-export from aion-types so callers can use a single import path
pub use aion_types::skill_types::{ContextModifier, effort_to_string};

/// Build a ContextModifier from skill metadata. Returns None if no overrides are specified.
pub fn from_skill(skill: &SkillMetadata) -> Option<ContextModifier> {
    let has_overrides = skill.model.is_some() || skill.effort.is_some() || !skill.allowed_tools.is_empty();

    if !has_overrides {
        return None;
    }

    Some(ContextModifier {
        model: skill.model.clone(),
        effort: skill.effort,
        allowed_tools: skill.allowed_tools.clone(),
        ..Default::default()
    })
}

#[cfg(test)]
#[path = "context_modifier_test.rs"]
mod context_modifier_test;
