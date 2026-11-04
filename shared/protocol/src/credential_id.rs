use std::{error::Error, fmt};

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct CredentialId(String);

impl CredentialId {
    pub fn parse(value: impl Into<String>) -> Result<Self, InvalidCredentialId> {
        let value = value.into();
        if value.len() != 32
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(InvalidCredentialId);
        }
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidCredentialId;

impl fmt::Display for InvalidCredentialId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("credential ID must be 32 lowercase hexadecimal characters")
    }
}
impl Error for InvalidCredentialId {}
impl fmt::Debug for CredentialId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("CredentialId")
            .field(&self.0)
            .finish()
    }
}
