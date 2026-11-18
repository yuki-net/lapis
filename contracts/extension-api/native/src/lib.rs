//! ネイティブ拡張ホストと拡張が共有する ABI v1。
//!
//! 同じプロセス・同じ target のモジュール間でのみ使う。操作と能力の意味は
//! それぞれの版付き契約が定め、境界を越える内容は UTF-8 の JSON とする。

use std::ffi::c_void;

pub const ABI_VERSION: u32 = 1;
pub const ENTRY_SYMBOL: &[u8] = b"lapis_extension_v1\0";
pub const STATUS_OK: i32 = 0;
pub const STATUS_ERROR: i32 = 1;
pub const MAX_MESSAGE_BYTES: usize = 64 * 1024;

/// 呼び出し中だけ有効な読み取り専用の byte 列。空は null と長さ 0 で表す。
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ByteView {
    pub ptr: *const u8,
    pub len: usize,
}

impl ByteView {
    pub fn from_bytes(bytes: &[u8]) -> Self {
        Self {
            ptr: if bytes.is_empty() {
                std::ptr::null()
            } else {
                bytes.as_ptr()
            },
            len: bytes.len(),
        }
    }
}

/// 確保した側の release 関数で必ず解放する。空は null と長さ 0 で表す。
#[repr(C)]
#[derive(Clone, Copy)]
pub struct OwnedBytes {
    pub ptr: *mut u8,
    pub len: usize,
}

impl OwnedBytes {
    pub const EMPTY: Self = Self {
        ptr: std::ptr::null_mut(),
        len: 0,
    };
}

/// Host の能力呼び出し。context は不透明なハンドルで、拡張が参照してはならない。
/// Host API と context は拡張を deactivate するまで有効とする。
#[repr(C)]
#[derive(Clone, Copy)]
pub struct HostApiV1 {
    pub abi_version: u32,
    pub struct_size: u32,
    pub context: *mut c_void,
    pub call: Option<unsafe extern "C" fn(*mut c_void, ByteView, ByteView, *mut OwnedBytes) -> i32>,
    pub release: Option<unsafe extern "C" fn(*mut c_void, OwnedBytes)>,
}

/// 拡張が `lapis_extension_v1` から返す関数表。
/// 呼び出しはインスタンス単位で直列化し、返却値は拡張側の release で解放する。
/// STATUS_ERROR の出力は利用者向けではない UTF-8 エラー文字列とする。
/// activate の失敗時はインスタンスを返さない。deactivate を呼んだ後は、
/// 成否にかかわらず同じインスタンスを再利用してはならない。
#[repr(C)]
pub struct ExtensionApiV1 {
    pub abi_version: u32,
    pub struct_size: u32,
    pub activate:
        Option<unsafe extern "C" fn(*const HostApiV1, *mut *mut c_void, *mut OwnedBytes) -> i32>,
    pub dispatch: Option<
        unsafe extern "C" fn(
            *mut c_void,
            ByteView,
            ByteView,
            ByteView,
            ByteView,
            *mut OwnedBytes,
        ) -> i32,
    >,
    pub deactivate: Option<unsafe extern "C" fn(*mut c_void, *mut OwnedBytes) -> i32>,
    pub release: Option<unsafe extern "C" fn(OwnedBytes)>,
}

pub type EntryV1 = unsafe extern "C" fn() -> *const ExtensionApiV1;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v1_entry_and_buffer_conventions_are_stable() {
        assert_eq!(ABI_VERSION, 1);
        assert_eq!(ENTRY_SYMBOL, b"lapis_extension_v1\0");
        assert_eq!(OwnedBytes::EMPTY.len, 0);
        assert!(OwnedBytes::EMPTY.ptr.is_null());
        let bytes = b"test";
        let view = ByteView::from_bytes(bytes);
        assert_eq!(view.ptr, bytes.as_ptr());
        assert_eq!(view.len, bytes.len());
    }
}
