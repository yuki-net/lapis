use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Revision(u64);

impl Revision {
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

macro_rules! sequence {
    ($name:ident) => {
        #[derive(
            Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize,
        )]
        #[serde(transparent)]
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
sequence!(TerminalOutputSequence);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counters_are_scoped_and_detect_overflow() {
        assert_eq!(Revision::new(4).checked_next().unwrap().value(), 5);
        assert_eq!(EventSequence::new(8).checked_next().unwrap().value(), 9);
        assert_eq!(
            TerminalOutputSequence::new(12)
                .checked_next()
                .unwrap()
                .value(),
            13
        );
        assert!(Revision::new(u64::MAX).checked_next().is_none());
        assert!(EventSequence::new(u64::MAX).checked_next().is_none());
        assert!(
            TerminalOutputSequence::new(u64::MAX)
                .checked_next()
                .is_none()
        );
    }
}
