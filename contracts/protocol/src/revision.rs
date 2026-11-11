use serde::{Deserialize, Serialize};

/// Document の版。定義と semantics は domain(`document`) を正本とする。
pub use ::document::Revision;

macro_rules! sequence {
    ($name:ident) => {
        #[derive(
            Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        /// 接続ごとの event 連番。取りこぼし検出に使う。
        pub struct $name(u64);

        impl $name {
            pub const fn new(value: u64) -> Self {
                Self(value)
            }

            pub const fn value(self) -> u64 {
                self.0
            }

            pub const fn checked_next(self) -> Option<Self> {
                match self.0.checked_add(1) {
                    Some(value) => Some(Self(value)),
                    None => None,
                }
            }
        }
    };
}

sequence!(EventSequence);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counters_are_scoped_and_handle_overflow() {
        assert_eq!(Revision::new(4).next().number(), 5);
        assert_eq!(EventSequence::new(8).checked_next().unwrap().value(), 9);
        // Revision は飽和して Document を dirty のまま保つ。
        assert_eq!(Revision::new(u64::MAX).next().number(), u64::MAX);
        assert!(EventSequence::new(u64::MAX).checked_next().is_none());
    }
}
