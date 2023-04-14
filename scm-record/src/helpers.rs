//! Helper functions for rendering UI components.

/// Generate a one-line description of a binary file change.
pub fn make_binary_description(hash: &str, num_bytes: u64) -> String {
    format!("{} ({} bytes)", hash, num_bytes)
}
