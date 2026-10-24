pub(super) struct IconAsset {
    pub(super) path: &'static str,
    pub(super) bytes: &'static [u8],
}

macro_rules! define_icons {
    ($( $name:ident => ($path:literal, $bytes:expr) ),+ $(,)?) => {
        /// アプリに同梱する固定アイコン。
        ///
        /// 拡張機能やテーマから提供される動的なアイコンは `crate::icons` が扱う。
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        #[allow(dead_code)] // 既存画面の置換は行わず、段階的に利用する。
        pub(crate) enum IconName {
            $( $name, )+
        }

        impl IconName {
            pub(super) const ALL: &[Self] = &[$( Self::$name, )+];

            pub(super) const fn asset(self) -> IconAsset {
                match self {
                    $( Self::$name => IconAsset { path: $path, bytes: $bytes }, )+
                }
            }
        }
    };
}

// 固定アイコンを追加するときは、SVGファイルを置き、この一覧へ1行追加する。
define_icons! {
    Terminal => ("icons/terminal.svg", include_bytes!("../../../assets/icons/terminal.svg")),
    Files => ("icons/files.svg", include_bytes!("../../../assets/icons/files.svg")),
    Git => ("icons/git.svg", include_bytes!("../../../assets/icons/git.svg")),
    Message => ("icons/message.svg", include_bytes!("../../../assets/icons/message.svg")),
    History => ("icons/history.svg", include_bytes!("../../../assets/icons/history.svg")),
    Preview => ("icons/preview.svg", include_bytes!("../../../assets/icons/preview.svg")),
    Problems => ("icons/problems.svg", include_bytes!("../../../assets/icons/problems.svg")),
    Menu => ("icons/menu.svg", include_bytes!("../../../assets/icons/menu.svg")),
    Search => ("icons/search.svg", include_bytes!("../../../assets/icons/search.svg")),
    Close => ("icons/close.svg", include_bytes!("../../../assets/icons/close.svg")),
    Minus => ("icons/minus.svg", include_bytes!("../../../assets/icons/minus.svg")),
    Square => ("icons/square.svg", include_bytes!("../../../assets/icons/square.svg")),
    X => ("icons/x.svg", include_bytes!("../../../assets/icons/x.svg")),
    PanelLeft => ("icons/panel-left.svg", include_bytes!("../../../assets/icons/panel-left.svg")),
    PanelLeftDashed => ("icons/panel-left-dashed.svg", include_bytes!("../../../assets/icons/panel-left-dashed.svg")),
    Settings => ("icons/settings.svg", include_bytes!("../../../assets/icons/settings.svg")),
    SunMoon => ("icons/sun-moon.svg", include_bytes!("../../../assets/icons/sun-moon.svg")),
}

impl IconName {
    pub(crate) fn for_tool(icon: &str) -> Self {
        match icon {
            "files" => Self::Files,
            "search" => Self::Search,
            "git" => Self::Git,
            "history" => Self::History,
            "preview" => Self::Preview,
            "assistant" | "tasks" => Self::Message,
            "terminal" | "output" => Self::Terminal,
            "problems" => Self::Problems,
            "settings" => Self::Settings,
            _ => Self::Square,
        }
    }
}
