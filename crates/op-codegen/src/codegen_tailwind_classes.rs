//! Ordered class-string ownership and one-buffer attribute emission.
/// Ordered, de-duplicated class list.
#[derive(Debug, Default, Clone)]
pub(in crate::codegen_tailwind) struct Classes(Vec<String>);

impl Classes {
    pub(in crate::codegen_tailwind) fn push(&mut self, class: impl Into<String>) {
        let class = class.into();
        if !class.is_empty() && !self.0.contains(&class) {
            self.0.push(class);
        }
    }

    pub(in crate::codegen_tailwind) fn push_opt(&mut self, class: Option<impl Into<String>>) {
        if let Some(class) = class {
            self.push(class);
        }
    }

    pub(in crate::codegen_tailwind) fn extend(&mut self, other: Classes) {
        for class in other.0 {
            self.push(class);
        }
    }

    pub(in crate::codegen_tailwind) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// ` className="…"` attribute (leading space), or empty.
    pub(in crate::codegen_tailwind) fn attr(&self) -> String {
        if self.0.is_empty() {
            String::new()
        } else {
            let capacity = self
                .0
                .iter()
                .try_fold(13usize.saturating_add(self.0.len() - 1), |total, class| {
                    total.checked_add(class.len())
                })
                .unwrap_or(0);
            let mut attribute = String::with_capacity(capacity);
            attribute.push_str(" className=\"");
            for (index, class) in self.0.iter().enumerate() {
                if index != 0 {
                    attribute.push(' ');
                }
                attribute.push_str(class);
            }
            attribute.push('"');
            attribute
        }
    }
}
