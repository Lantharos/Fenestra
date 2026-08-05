//! Guest input-policy helpers shared with the CEF host contract.
//!
//! The live interception runs in the C++ OSR host. This module mirrors the
//! accelerator and wheel rules so they can be unit-tested from Rust.

/// CEF event-flag bits used for guest shortcut / wheel payloads.
pub const MOD_SHIFT: u32 = 1 << 1;
pub const MOD_CONTROL: u32 = 1 << 2;
pub const MOD_ALT: u32 = 1 << 3;
pub const MOD_COMMAND: u32 = 1 << 7;
pub const MOD_MASK: u32 = MOD_SHIFT | MOD_CONTROL | MOD_ALT | MOD_COMMAND;

/// Platform meaning of the `Primary` modifier token.
pub fn platform_primary_modifier() -> u32 {
    if cfg!(target_os = "macos") {
        MOD_COMMAND
    } else {
        MOD_CONTROL
    }
}

fn normalize_shortcut_key(key: &str) -> String {
    let mut key = key.to_ascii_lowercase();
    if key.starts_with("key") && key.len() == 4 {
        key = key[3..].to_string();
    }
    if key == " " || key == "space" {
        return "space".to_string();
    }
    key
}

fn parse_accelerator(accelerator: &str) -> Option<(u32, String)> {
    let mut parts: Vec<&str> = accelerator
        .split('+')
        .filter(|part| !part.is_empty())
        .collect();
    let key = normalize_shortcut_key(parts.pop()?);
    if key.is_empty() {
        return None;
    }
    let mut mods = 0u32;
    for part in parts {
        match part.to_ascii_lowercase().as_str() {
            "primary" => mods |= platform_primary_modifier(),
            "control" | "ctrl" => mods |= MOD_CONTROL,
            "meta" | "command" | "cmd" => mods |= MOD_COMMAND,
            "alt" | "option" => mods |= MOD_ALT,
            "shift" => mods |= MOD_SHIFT,
            _ => return None,
        }
    }
    Some((mods, key))
}

/// Returns the matching accelerator when `key` + `modifiers` match an entry.
pub fn match_intercepted_shortcut<'a>(
    shortcuts: &'a [String],
    key: &str,
    modifiers: u32,
) -> Option<&'a str> {
    let normalized_key = normalize_shortcut_key(key);
    let pressed = modifiers & MOD_MASK;
    shortcuts.iter().find_map(|accelerator| {
        let (required_mods, required_key) = parse_accelerator(accelerator)?;
        (required_key == normalized_key && required_mods == pressed).then_some(accelerator.as_str())
    })
}

/// Argent threshold for predominantly horizontal wheel samples.
pub fn is_predominantly_horizontal_wheel(delta_x: f64, delta_y: f64) -> bool {
    let abs_x = delta_x.abs();
    let abs_y = delta_y.abs();
    abs_x > 0.75 && abs_x >= abs_y * 0.9
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primary_plus_k_matches_platform_primary() {
        let shortcuts = vec!["Primary+K".to_string()];
        assert_eq!(
            match_intercepted_shortcut(&shortcuts, "k", platform_primary_modifier()),
            Some("Primary+K")
        );
        assert_eq!(
            match_intercepted_shortcut(&shortcuts, "KeyK", platform_primary_modifier()),
            Some("Primary+K")
        );
        assert!(match_intercepted_shortcut(&shortcuts, "k", 0).is_none());
        assert!(
            match_intercepted_shortcut(&shortcuts, "k", platform_primary_modifier() | MOD_SHIFT)
                .is_none()
        );
    }

    #[test]
    fn explicit_control_does_not_become_meta() {
        let shortcuts = vec!["Control+T".to_string()];
        assert_eq!(
            match_intercepted_shortcut(&shortcuts, "t", MOD_CONTROL),
            Some("Control+T")
        );
        assert!(match_intercepted_shortcut(&shortcuts, "t", MOD_COMMAND).is_none());
    }

    #[test]
    fn horizontal_wheel_threshold() {
        assert!(is_predominantly_horizontal_wheel(10.0, 5.0));
        assert!(is_predominantly_horizontal_wheel(1.0, 1.0));
        assert!(!is_predominantly_horizontal_wheel(0.5, 0.0));
        assert!(!is_predominantly_horizontal_wheel(10.0, 20.0));
    }
}
