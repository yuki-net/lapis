use std::{fs, io, path::Path};

use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use zeroize::Zeroizing;

use crate::protected::protect;

/// 保存形式の識別子。形式を変えたら末尾の版を上げる。
const MAGIC: &[u8] = b"lapis-host-identity\0\x01";
const ENTROPY: &[u8] = b"lapis-host-identity";
const FILE: &str = "identity";
const MAX_SIZE: u64 = 64 * 1024;

type Error = Box<dyn std::error::Error + Send + Sync>;

/// Hostの同一性。DesktopのprofileとWebのcredentialは証明書に紐づくため、再起動を跨いで使い回す。
pub struct HostIdentity {
    pub certificate: CertificateDer<'static>,
    pub key: PrivateKeyDer<'static>,
}

impl HostIdentity {
    /// state directoryがあれば保存済みの同一性を再利用し、無ければメモリ内だけで生成する。
    pub fn load_or_create(state: Option<&Path>) -> Result<Self, Error> {
        let Some(state) = state else {
            return Self::generate();
        };
        let path = state.join(FILE);
        if path.is_file() {
            match Self::read(&path) {
                Ok(identity) => return Ok(identity),
                // 読めない保存物は作り直す。この場合だけ証明書が変わり、再接続にペアリングを要する。
                Err(error) => eprintln!("Hostの同一性を使い回せないため作り直します: {error}"),
            }
        }
        let identity = Self::generate()?;
        identity.write(&path)?;
        Ok(identity)
    }

    fn generate() -> Result<Self, Error> {
        let generated = rcgen::generate_simple_self_signed(vec![
            "localhost".to_owned(),
            "lapis-host".to_owned(),
        ])?;
        let key = Zeroizing::new(generated.signing_key.serialize_der());
        Ok(Self {
            certificate: generated.cert.der().clone(),
            key: PrivateKeyDer::Pkcs8(key.to_vec().into()),
        })
    }

    fn read(path: &Path) -> Result<Self, Error> {
        let encrypted = Zeroizing::new(fs::read(path)?);
        if encrypted.len() as u64 > MAX_SIZE {
            return Err("Hostの同一性ファイルが大きすぎます".into());
        }
        let plaintext = protect(&encrypted, ENTROPY, true)?;
        let body = plaintext
            .strip_prefix(MAGIC)
            .ok_or("Hostの同一性ファイルの形式が不正です")?;
        if body.len() < 4 {
            return Err("Hostの同一性ファイルの形式が不正です".into());
        }
        let (length, body) = body.split_at(4);
        let length = u32::from_le_bytes(length.try_into()?) as usize;
        if body.len() <= length {
            return Err("Hostの同一性ファイルの形式が不正です".into());
        }
        let (certificate, key) = body.split_at(length);
        Ok(Self {
            certificate: CertificateDer::from(certificate.to_vec()),
            key: PrivateKeyDer::Pkcs8(key.to_vec().into()),
        })
    }

    fn write(&self, path: &Path) -> Result<(), Error> {
        let directory = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .ok_or("Hostの同一性の保存先が不正です")?;
        fs::create_dir_all(directory)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;
        }
        let certificate = self.certificate.as_ref();
        let mut plaintext =
            Zeroizing::new(Vec::with_capacity(MAGIC.len() + 4 + certificate.len() + 64));
        plaintext.extend_from_slice(MAGIC);
        let length = u32::try_from(certificate.len())
            .map_err(|_| "Hostの証明書が大きすぎます")?
            .to_le_bytes();
        plaintext.extend_from_slice(&length);
        plaintext.extend_from_slice(certificate);
        plaintext.extend_from_slice(self.key.secret_der());
        let encrypted = protect(&plaintext, ENTROPY, false)?;
        // 途中の半端な内容を読ませないよう、同じディレクトリの一時ファイルから置き換える。
        let mut file = tempfile::NamedTempFile::new_in(directory)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.as_file()
                .set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        io::Write::write_all(file.as_file_mut(), &encrypted)?;
        io::Write::flush(file.as_file_mut())?;
        file.as_file().sync_all()?;
        file.persist(path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_is_reused_and_not_stored_in_plaintext() {
        let directory = tempfile::tempdir().unwrap();
        let first = HostIdentity::load_or_create(Some(directory.path())).unwrap();
        let second = HostIdentity::load_or_create(Some(directory.path())).unwrap();
        assert_eq!(first.certificate.as_ref(), second.certificate.as_ref());
        assert_eq!(first.key.secret_der(), second.key.secret_der());
        let stored = fs::read(directory.path().join(FILE)).unwrap();
        let key = first.key.secret_der();
        assert!(!stored.windows(key.len()).any(|window| window == key));
    }

    #[test]
    fn broken_identity_is_regenerated() {
        let directory = tempfile::tempdir().unwrap();
        let first = HostIdentity::load_or_create(Some(directory.path())).unwrap();
        fs::write(directory.path().join(FILE), b"broken").unwrap();
        let second = HostIdentity::load_or_create(Some(directory.path())).unwrap();
        assert_ne!(first.certificate.as_ref(), second.certificate.as_ref());
    }

    #[test]
    fn identity_without_state_directory_is_ephemeral() {
        let first = HostIdentity::load_or_create(None).unwrap();
        let second = HostIdentity::load_or_create(None).unwrap();
        assert_ne!(first.certificate.as_ref(), second.certificate.as_ref());
    }
}
