//! A sealed completed-state proof, not a command-name or revision prediction.
use jian_ops_schema::node::PenNode;

#[derive(Clone, Copy)]
pub struct CanonicalRefTarget<'a> {
    old: &'a PenNode,
    current: &'a PenNode,
    before_target: &'a str,
    after_target: &'a str,
}
impl<'a> CanonicalRefTarget<'a> {
    pub(crate) fn certified(
        old: &'a PenNode,
        current: &'a PenNode,
        before_target: &'a str,
        after_target: &'a str,
    ) -> Self {
        Self {
            old,
            current,
            before_target,
            after_target,
        }
    }
    pub fn old(self) -> &'a PenNode {
        self.old
    }
    pub fn current(self) -> &'a PenNode {
        self.current
    }
    pub fn before_target(self) -> &'a str {
        self.before_target
    }
    pub fn after_target(self) -> &'a str {
        self.after_target
    }
}
