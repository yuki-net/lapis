use std::io;
use zeroize::Zeroizing;

#[cfg(windows)]
pub(super) fn protect(
    bytes: &[u8],
    entropy: &[u8],
    decrypt: bool,
) -> io::Result<Zeroizing<Vec<u8>>> {
    use windows_sys::Win32::{
        Foundation::LocalFree,
        Security::Cryptography::{
            CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
        },
    };
    let input = CRYPT_INTEGER_BLOB {
        cbData: bytes
            .len()
            .try_into()
            .map_err(|_| io::ErrorKind::InvalidInput)?,
        pbData: bytes.as_ptr().cast_mut(),
    };
    let salt = CRYPT_INTEGER_BLOB {
        cbData: entropy
            .len()
            .try_into()
            .map_err(|_| io::ErrorKind::InvalidInput)?,
        pbData: entropy.as_ptr().cast_mut(),
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };
    // user scopeのDPAPIで接続先・証明書・credentialをまとめて保護する。
    // LOCAL_MACHINEは使わず、入力は呼出中有効なslice、出力はLocalFreeで解放する。
    let success = unsafe {
        if decrypt {
            CryptUnprotectData(
                &input,
                std::ptr::null_mut(),
                &salt,
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        } else {
            CryptProtectData(
                &input,
                std::ptr::null(),
                &salt,
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        }
    };
    if success == 0 {
        return Err(io::Error::last_os_error());
    }
    let result = unsafe {
        Zeroizing::new(std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec())
    };
    unsafe {
        use zeroize::Zeroize;
        std::slice::from_raw_parts_mut(output.pbData, output.cbData as usize).zeroize();
        LocalFree(output.pbData.cast());
    }
    Ok(result)
}

#[cfg(unix)]
pub(super) fn protect(bytes: &[u8], _: &[u8], _: bool) -> io::Result<Zeroizing<Vec<u8>>> {
    // Unixの開発用Hostでは呼出側がdirectory 0700 / file 0600を強制する。
    Ok(Zeroizing::new(bytes.to_vec()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn credential_record_roundtrips() {
        let encrypted = protect(b"private credential", b"workspace", false).unwrap();
        assert_eq!(
            protect(&encrypted, b"workspace", true).unwrap().as_slice(),
            b"private credential"
        );
        #[cfg(windows)]
        {
            assert_ne!(encrypted.as_slice(), b"private credential");
            assert!(protect(&encrypted, b"different workspace", true).is_err());
        }
    }
}
