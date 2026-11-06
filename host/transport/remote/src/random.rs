use std::{error::Error, fmt};

/// 暗号学的乱数の境界。テストでは決定的な実装に差し替える。
pub trait RandomSource {
    fn fill_bytes(&mut self, destination: &mut [u8]) -> Result<(), RandomError>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct OsRandom;

impl RandomSource for OsRandom {
    fn fill_bytes(&mut self, destination: &mut [u8]) -> Result<(), RandomError> {
        getrandom::fill(destination).map_err(|_| RandomError)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct RandomError;

impl fmt::Display for RandomError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("secure random generation failed")
    }
}

impl Error for RandomError {}
