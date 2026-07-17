use std::ops::Range;

/// UIやファイルI/Oに依存しないテキストバッファ。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TextBuffer {
    content: String,
}

impl TextBuffer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_string(content: String) -> Self {
        Self { content }
    }

    pub fn as_str(&self) -> &str {
        &self.content
    }

    pub fn replace_range(&mut self, range: Range<usize>, replacement: &str) -> bool {
        if self.content[range.clone()] == *replacement {
            return false;
        }
        self.content.replace_range(range, replacement);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replacement_reports_only_real_changes() {
        let mut buffer = TextBuffer::from_string("Lapis".to_owned());

        assert!(!buffer.replace_range(0..5, "Lapis"));
        assert!(buffer.replace_range(0..5, "Editor"));
        assert_eq!(buffer.as_str(), "Editor");
    }
}
