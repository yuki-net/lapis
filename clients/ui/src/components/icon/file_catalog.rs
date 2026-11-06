pub(super) struct FileIconAsset {
    pub(super) path: &'static str,
    pub(super) bytes: &'static [u8],
}

macro_rules! define_file_icons {
    ($( $name:ident => ($path:literal, $bytes:expr) ),+ $(,)?) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub(crate) enum FileIconId {
            $( $name, )+
        }

        impl FileIconId {
            pub(super) const ALL: &[Self] = &[$( Self::$name, )+];

            pub(super) const fn asset(self) -> FileIconAsset {
                match self {
                    $( Self::$name => FileIconAsset { path: $path, bytes: $bytes }, )+
                }
            }
        }
    };
}

define_file_icons! {
    TextAlignStart => (
        "file-icons/text-align-start.svg",
        include_bytes!("../../../assets/icons/file/text-align-start.svg")
    ),
    Javascript => (
        "file-icons/javascript.svg",
        include_bytes!("../../../assets/icons/file/javascript.svg")
    ),
    Typescript => (
        "file-icons/typescript.svg",
        include_bytes!("../../../assets/icons/file/typescript.svg")
    ),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_file_icon_has_a_bundled_asset() {
        for &icon in FileIconId::ALL {
            assert!(!icon.asset().bytes.is_empty());
            assert!(icon.asset().path.starts_with("file-icons/"));
        }
    }
}
