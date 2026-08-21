use std::{error::Error, fmt};

use serde::{Deserialize, Serialize};

pub const CURRENT_PROTOCOL: ProtocolVersion = ProtocolVersion::new(1, 0);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ProtocolVersion {
    pub major: u16,
    pub minor: u16,
}

impl ProtocolVersion {
    pub const fn new(major: u16, minor: u16) -> Self {
        Self { major, minor }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "UncheckedProtocolRange")]
pub struct ProtocolRange {
    major: u16,
    minimum_minor: u16,
    maximum_minor: u16,
}

impl ProtocolRange {
    pub const fn try_new(
        major: u16,
        minimum_minor: u16,
        maximum_minor: u16,
    ) -> Result<Self, InvalidProtocolRange> {
        if minimum_minor > maximum_minor {
            return Err(InvalidProtocolRange);
        }
        Ok(Self {
            major,
            minimum_minor,
            maximum_minor,
        })
    }

    pub const fn exact(version: ProtocolVersion) -> Self {
        Self {
            major: version.major,
            minimum_minor: version.minor,
            maximum_minor: version.minor,
        }
    }

    pub const fn major(self) -> u16 {
        self.major
    }

    pub const fn minimum_minor(self) -> u16 {
        self.minimum_minor
    }

    pub const fn maximum_minor(self) -> u16 {
        self.maximum_minor
    }

    pub fn negotiate(self, peer: Self) -> Result<ProtocolVersion, VersionMismatch> {
        if self.major != peer.major {
            return Err(VersionMismatch::new(self, peer));
        }
        let minimum = self.minimum_minor.max(peer.minimum_minor);
        let maximum = self.maximum_minor.min(peer.maximum_minor);
        if minimum > maximum {
            return Err(VersionMismatch::new(self, peer));
        }
        Ok(ProtocolVersion::new(self.major, maximum))
    }
}

#[derive(Deserialize)]
struct UncheckedProtocolRange {
    major: u16,
    minimum_minor: u16,
    maximum_minor: u16,
}

impl TryFrom<UncheckedProtocolRange> for ProtocolRange {
    type Error = InvalidProtocolRange;

    fn try_from(value: UncheckedProtocolRange) -> Result<Self, Self::Error> {
        Self::try_new(value.major, value.minimum_minor, value.maximum_minor)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidProtocolRange;

impl fmt::Display for InvalidProtocolRange {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("protocol minimum minor must not exceed maximum minor")
    }
}

impl Error for InvalidProtocolRange {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VersionMismatch {
    local: ProtocolRange,
    peer: ProtocolRange,
}

impl VersionMismatch {
    const fn new(local: ProtocolRange, peer: ProtocolRange) -> Self {
        Self { local, peer }
    }

    pub const fn local(self) -> ProtocolRange {
        self.local
    }

    pub const fn peer(self) -> ProtocolRange {
        self.peer
    }
}

impl fmt::Display for VersionMismatch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "protocol ranges do not overlap: local {}.{}-{}, peer {}.{}-{}",
            self.local.major,
            self.local.minimum_minor,
            self.local.maximum_minor,
            self.peer.major,
            self.peer.minimum_minor,
            self.peer.maximum_minor
        )
    }
}

impl Error for VersionMismatch {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn negotiation_selects_the_highest_shared_minor() {
        let local = ProtocolRange::try_new(1, 0, 4).unwrap();
        let peer = ProtocolRange::try_new(1, 2, 3).unwrap();

        assert_eq!(local.negotiate(peer), Ok(ProtocolVersion::new(1, 3)));
    }

    #[test]
    fn negotiation_rejects_major_and_minor_mismatch() {
        assert!(
            ProtocolRange::exact(ProtocolVersion::new(1, 0))
                .negotiate(ProtocolRange::exact(ProtocolVersion::new(2, 0)))
                .is_err()
        );
        assert!(
            ProtocolRange::try_new(1, 0, 1)
                .unwrap()
                .negotiate(ProtocolRange::try_new(1, 2, 3).unwrap())
                .is_err()
        );
    }

    #[test]
    fn invalid_range_is_rejected_before_and_during_deserialization() {
        assert!(ProtocolRange::try_new(1, 3, 2).is_err());
        assert!(
            serde_json::from_str::<ProtocolRange>(
                r#"{"major":1,"minimum_minor":3,"maximum_minor":2}"#
            )
            .is_err()
        );
    }
}
