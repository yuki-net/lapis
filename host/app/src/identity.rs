use std::{fs, io, path::Path};

use rcgen::{CertificateParams, KeyPair};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use time::{Duration, OffsetDateTime};
use zeroize::Zeroizing;

use crate::protected::protect;

/// 保存形式の識別子。形式を変えたら末尾の版を上げる。
/// v2 は `not_after`（有効期限）を含む。
const MAGIC: &[u8] = b"lapis-host-identity\0\x02";
const ENTROPY: &[u8] = b"lapis-host-identity";
const FILE: &str = "identity";
const MAX_SIZE: u64 = 64 * 1024;

/// WebTransport の `serverCertificateHashes` は有効期間 14 日以内の証明書しか受け付けない。
/// 余裕を持って 13 日にする。
const CERTIFICATE_LIFETIME_DAYS: i64 = 13;
/// 端末とホストの時計ずれで `notBefore` より前になるのを避けるため、発行時刻を戻す。
const NOT_BEFORE_BACKDATE_MINUTES: i64 = 60;
/// 残り有効期間がこれを切ったら、起動時に作り直す。
const RENEW_BEFORE_DAYS: i64 = 1;

/// 証明書の有効期間。14日制約を超えないことはテストで確認する。
pub fn certificate_lifetime() -> Duration {
    Duration::days(CERTIFICATE_LIFETIME_DAYS)
}

type Error = Box<dyn std::error::Error + Send + Sync>;

/// Hostの同一性。DesktopのprofileとWebのcredentialは証明書に紐づくため、再起動を跨いで使い回す。
pub struct HostIdentity {
    pub certificate: CertificateDer<'static>,
    pub key: PrivateKeyDer<'static>,
    /// 証明書の有効期限。起動時に更新が必要かを判断する。
    pub not_after: OffsetDateTime,
}

impl HostIdentity {
    /// state directoryがあれば保存済みの同一性を再利用し、無ければメモリ内だけで生成する。
    pub fn load_or_create(state: Option<&Path>) -> Result<Self, Error> {
        Self::load_or_create_at(state, OffsetDateTime::now_utc())
    }

    /// 現在時刻を注入して、保存済みの証明書を使い回すか作り直すかを判断する。
    pub fn load_or_create_at(state: Option<&Path>, now: OffsetDateTime) -> Result<Self, Error> {
        let Some(state) = state else {
            return Self::generate(now);
        };
        let path = state.join(FILE);
        if path.is_file() {
            match Self::read(&path) {
                Ok(identity) if !identity.needs_renewal(now) => return Ok(identity),
                Ok(_) => eprintln!("Hostの証明書の有効期間が残りわずかなため作り直します"),
                // 読めない保存物は作り直す。この場合だけ証明書が変わり、再接続にペアリングを要する。
                Err(error) => eprintln!("Hostの同一性を使い回せないため作り直します: {error}"),
            }
        }
        let identity = Self::generate(now)?;
        identity.write(&path)?;
        Ok(identity)
    }

    /// WebTransport と通常の QUIC が共通で使う証明書を作る。
    ///
    /// ブラウザーは ECDSA 鍵の短期証明書だけを `serverCertificateHashes` で受け付けるため、
    /// 鍵種を P-256 に固定し、有効期間を 13 日にする。
    fn generate(now: OffsetDateTime) -> Result<Self, Error> {
        let key_pair = KeyPair::generate()?;
        let mut params = CertificateParams::new(vec![
            "localhost".to_owned(),
            "lapis-host".to_owned(),
        ])?;
        params.not_before = now - Duration::minutes(NOT_BEFORE_BACKDATE_MINUTES);
        params.not_after = params.not_before + certificate_lifetime();
        let certificate = params.self_signed(&key_pair)?;
        Ok(Self {
            certificate: certificate.der().clone(),
            key: PrivateKeyDer::Pkcs8(Zeroizing::new(key_pair.serialize_der()).to_vec().into()),
            not_after: params.not_after,
        })
    }

    /// 残り有効期間が短い証明書は、ブラウザーが拒否する前に作り直す。
    fn needs_renewal(&self, now: OffsetDateTime) -> bool {
        self.not_after - now < Duration::days(RENEW_BEFORE_DAYS)
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
        // 証明書長(4) + 有効期限の unix 秒(8) + 証明書 + 鍵
        if body.len() < 12 {
            return Err("Hostの同一性ファイルの形式が不正です".into());
        }
        let (length, body) = body.split_at(4);
        let length = u32::from_le_bytes(length.try_into()?) as usize;
        let (not_after, body) = body.split_at(8);
        let not_after =
            OffsetDateTime::from_unix_timestamp(i64::from_le_bytes(not_after.try_into()?))
                .map_err(|_| "Hostの証明書の有効期限が不正です")?;
        if body.len() <= length {
            return Err("Hostの同一性ファイルの形式が不正です".into());
        }
        let (certificate, key) = body.split_at(length);
        Ok(Self {
            certificate: CertificateDer::from(certificate.to_vec()),
            key: PrivateKeyDer::Pkcs8(key.to_vec().into()),
            not_after,
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
            Zeroizing::new(Vec::with_capacity(MAGIC.len() + 12 + certificate.len() + 64));
        plaintext.extend_from_slice(MAGIC);
        let length = u32::try_from(certificate.len())
            .map_err(|_| "Hostの証明書が大きすぎます")?
            .to_le_bytes();
        plaintext.extend_from_slice(&length);
        plaintext.extend_from_slice(&self.not_after.unix_timestamp().to_le_bytes());
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

    #[test]
    fn certificate_stays_within_the_browser_webtransport_limit() {
        let now = OffsetDateTime::now_utc();
        let identity = HostIdentity::load_or_create_at(None, now).unwrap();
        // Chrome は 14 日超の証明書を serverCertificateHashes で受け付けない。
        assert!(identity.not_after - now <= Duration::days(14));
        assert!(identity.not_after > now);
        // 端末の時計が少し遅れていても notBefore より後になる。
        assert!(identity.not_after - certificate_lifetime() < now);
    }

    #[test]
    fn expiry_is_persisted_and_only_short_remaining_lifetime_renews() {
        let directory = tempfile::tempdir().unwrap();
        let now = OffsetDateTime::now_utc();
        let first = HostIdentity::load_or_create_at(Some(directory.path()), now).unwrap();
        // 有効期間の途中で再起動しても同じ証明書を使う。
        let middle = HostIdentity::load_or_create_at(
            Some(directory.path()),
            now + Duration::days(6),
        )
        .unwrap();
        assert_eq!(first.certificate.as_ref(), middle.certificate.as_ref());
        // 有効期限は秒単位で保存する。
        assert_eq!(first.not_after.unix_timestamp(), middle.not_after.unix_timestamp());
        // 残り 1 日を切ったら作り直し、有効期間を延ばす。
        let renewed = HostIdentity::load_or_create_at(
            Some(directory.path()),
            now + Duration::days(13),
        )
        .unwrap();
        assert_ne!(first.certificate.as_ref(), renewed.certificate.as_ref());
        assert!(renewed.not_after > now + Duration::days(20));
    }
}
