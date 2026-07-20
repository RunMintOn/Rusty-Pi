//! Picker trait — abstraction over `inquire` for testable interactive selectors.
//!
//! All picker operations work with `String` values to keep the trait simple
//! and mockable. Production uses [`inquire`]; tests use [`MockPicker`].

/// Trait for interactive prompting (select, text input).
pub trait Picker: Send + Sync {
    /// Show a single-select prompt with `String` options.
    /// Returns the selected value, or an error if cancelled.
    fn select(&self, prompt: &str, options: Vec<String>) -> anyhow::Result<String>;

    /// Show a text input prompt.
    fn text(&self, prompt: &str, default: Option<&str>, help_message: Option<&str>) -> anyhow::Result<String>;
}

/// Production picker backed by `inquire`.
pub struct RealPicker;

impl Picker for RealPicker {
    fn select(&self, prompt: &str, options: Vec<String>) -> anyhow::Result<String> {
        let selected = inquire::Select::new(prompt, options)
            .prompt()
            .map_err(|e| anyhow::anyhow!("Selection cancelled: {}", e))?;
        Ok(selected)
    }

    fn text(&self, prompt: &str, default: Option<&str>, help_message: Option<&str>) -> anyhow::Result<String> {
        let mut text = inquire::Text::new(prompt);
        if let Some(d) = default {
            text = text.with_default(d);
        }
        if let Some(h) = help_message {
            text = text.with_help_message(h);
        }
        text.prompt().map_err(|e| anyhow::anyhow!("Input cancelled: {}", e))
    }
}

/// Mock picker for testing — returns pre-configured values.
pub struct MockPicker {
    pub select_values: Vec<String>,
    pub text_values: Vec<String>,
    select_idx: std::sync::atomic::AtomicUsize,
    text_idx: std::sync::atomic::AtomicUsize,
}

impl MockPicker {
    pub fn new(select_values: Vec<String>, text_values: Vec<String>) -> Self {
        Self {
            select_values,
            text_values,
            select_idx: std::sync::atomic::AtomicUsize::new(0),
            text_idx: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    fn next_idx(idx: &std::sync::atomic::AtomicUsize, max: usize) -> Option<usize> {
        let current = idx.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if current < max { Some(current) } else { None }
    }
}

impl Picker for MockPicker {
    fn select(&self, _prompt: &str, _options: Vec<String>) -> anyhow::Result<String> {
        let idx = Self::next_idx(&self.select_idx, self.select_values.len())
            .ok_or_else(|| anyhow::anyhow!("MockPicker: no more select values"))?;
        Ok(self.select_values[idx].clone())
    }

    fn text(&self, _prompt: &str, _default: Option<&str>, _help_message: Option<&str>) -> anyhow::Result<String> {
        let idx = Self::next_idx(&self.text_idx, self.text_values.len())
            .ok_or_else(|| anyhow::anyhow!("MockPicker: no more text values"))?;
        Ok(self.text_values[idx].clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_picker_returns_select_values_in_order() {
        let picker = MockPicker::new(vec!["model-a".into(), "model-b".into()], vec![]);
        assert_eq!(picker.select("pick", vec!["a".into(), "b".into()]).unwrap(), "model-a");
        assert_eq!(picker.select("pick", vec!["a".into(), "b".into()]).unwrap(), "model-b");
    }

    #[test]
    fn mock_picker_returns_text_values() {
        let picker = MockPicker::new(vec![], vec!["/path/to/file".into()]);
        assert_eq!(picker.text("path?", None, None).unwrap(), "/path/to/file");
    }

    #[test]
    fn mock_picker_errors_when_exhausted() {
        let picker = MockPicker::new(vec![], vec![]);
        assert!(picker.select("pick", vec!["a".into()]).is_err());
    }
}
