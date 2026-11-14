//! Opaque identity for moving an already admitted call to a commit leader.
use std::thread::ThreadId;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AdmissionOwner {
    pub(crate) thread: ThreadId,
    pub(crate) ticket: u64,
}

#[derive(Debug)]
pub(super) struct Identity {
    store: usize,
    agent: String,
    owner: AdmissionOwner,
}

impl Identity {
    pub(super) fn new(store: usize, agent: String, owner: AdmissionOwner) -> Self {
        Self {
            store,
            agent,
            owner,
        }
    }
    pub(super) fn agent(&self) -> &str {
        &self.agent
    }
    pub(super) fn allows(&self, store: usize, agent: &str, owner: AdmissionOwner) -> bool {
        self.store == store && self.agent == agent && self.owner == owner
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opaque_permit_matches_store_agent_and_unique_admission() {
        let owner = AdmissionOwner {
            thread: std::thread::current().id(),
            ticket: 7,
        };
        let identity = Identity::new(17, "agent".into(), owner);
        assert!(identity.allows(17, "agent", owner));
        assert!(!identity.allows(18, "agent", owner));
        assert!(!identity.allows(17, "other", owner));
        assert!(!identity.allows(17, "agent", AdmissionOwner { ticket: 8, ..owner }));
    }

    #[test]
    fn a_leader_can_use_only_the_still_active_original_admission() {
        let owner = AdmissionOwner {
            thread: std::thread::current().id(),
            ticket: 19,
        };
        let identity = Identity::new(1, "a".into(), owner);
        std::thread::scope(|scope| {
            scope
                .spawn(|| {
                    let leader = std::thread::current().id();
                    assert_ne!(leader, owner.thread);
                    assert!(identity.allows(1, "a", owner));
                    assert!(!identity.allows(
                        1,
                        "a",
                        AdmissionOwner {
                            thread: leader,
                            ticket: 19
                        }
                    ));
                })
                .join()
                .unwrap();
        });
    }
}
