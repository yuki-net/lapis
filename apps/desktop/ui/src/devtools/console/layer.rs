//! tracing::Event を DiagnosticLogEntry へ変換する Layer。
//!
//! tracing-subscriber の Layer として動作し、イベントを bounded store に書き込む。
//! GPUI の Global として登録された DiagnosticLogStore を std::sync::Arc で共有する。

use std::sync::Arc;
use tracing::Event;
use tracing_subscriber::Layer;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use super::entry::{DiagnosticLevel, DiagnosticLogEntry, DiagnosticSource};
use super::store::DiagnosticLogStore;

/// 診断ログ Layer。
///
/// tracing::Event を DiagnosticLogEntry へ変換し、DiagnosticLogStore に書き込む。
pub(crate) struct DiagnosticLayer {
    store: Arc<DiagnosticLogStore>,
}

impl DiagnosticLayer {
    pub(crate) fn new(store: Arc<DiagnosticLogStore>) -> Self {
        Self { store }
    }
}

impl<S: tracing::Subscriber> Layer<S> for DiagnosticLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: tracing_subscriber::layer::Context<'_, S>) {
        let mut visitor = FieldVisitor::default();
        event.record(&mut visitor);

        let level = DiagnosticLevel::from_tracing(*event.metadata().level());
        let target = event.metadata().target().to_string();
        let message = if visitor.message.is_empty() {
            target.clone()
        } else {
            visitor.message
        };

        let sequence = self.store.next_sequence();
        let entry = DiagnosticLogEntry {
            sequence,
            occurred_at: std::time::SystemTime::now(),
            level,
            source: DiagnosticSource::Desktop,
            target,
            message,
            fields: serde_json::Value::Object(visitor.fields),
        };
        self.store.push(entry);
    }
}

/// tracing のフィールドを serde_json::Value へ変換する Visitor。
struct FieldVisitor {
    message: String,
    fields: serde_json::Map<String, serde_json::Value>,
}

impl Default for FieldVisitor {
    fn default() -> Self {
        Self {
            message: String::new(),
            fields: serde_json::Map::new(),
        }
    }
}

impl tracing::field::Visit for FieldVisitor {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "message" {
            self.message = value.to_string();
        } else {
            self.fields.insert(
                field.name().to_string(),
                serde_json::Value::String(value.to_string()),
            );
        }
    }

    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.message = format!("{:?}", value);
        } else {
            self.fields.insert(
                field.name().to_string(),
                serde_json::Value::String(format!("{:?}", value)),
            );
        }
    }
}

/// tracing subscriber を初期化し、DiagnosticLayer を返す。
///
/// この関数は1回だけ呼び出すこと。2回呼び出すと panic する。
/// 返された Arc<DiagnosticLogStore> を GPUI の Global として登録する。
pub(crate) fn init_subscriber() -> Arc<DiagnosticLogStore> {
    let store = Arc::new(DiagnosticLogStore::default());
    let layer = DiagnosticLayer::new(store.clone());

    // tracing_subscriber::set_global_default は2回呼び出すと panic する。
    // テスト環境でも安全に動作するよう、try_init を使用する。
    let _ = tracing_subscriber::registry().with(layer).try_init();

    store
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostic_level_from_tracing() {
        assert_eq!(
            DiagnosticLevel::from_tracing(tracing::Level::ERROR),
            DiagnosticLevel::Error
        );
        assert_eq!(
            DiagnosticLevel::from_tracing(tracing::Level::WARN),
            DiagnosticLevel::Warn
        );
        assert_eq!(
            DiagnosticLevel::from_tracing(tracing::Level::INFO),
            DiagnosticLevel::Info
        );
        assert_eq!(
            DiagnosticLevel::from_tracing(tracing::Level::DEBUG),
            DiagnosticLevel::Debug
        );
        assert_eq!(
            DiagnosticLevel::from_tracing(tracing::Level::TRACE),
            DiagnosticLevel::Trace
        );
    }

    #[test]
    fn layer_converts_event_to_entry() {
        let store = Arc::new(DiagnosticLogStore::default());
        let layer = DiagnosticLayer::new(store.clone());

        // tracing-subscriber のテスト用 subscriber で Layer を動作させる
        let subscriber = tracing_subscriber::registry().with(layer);
        let _guard = tracing::subscriber::set_default(subscriber);

        tracing::info!(
            message = "test message",
            key = "value",
            target = "lapis::test"
        );
        // 注意: target は metadata の target で、フィールドの target とは異なる

        let entries = store.entries();
        assert!(!entries.is_empty());
        let entry = &entries[0];
        assert_eq!(entry.level, DiagnosticLevel::Info);
        assert_eq!(entry.message, "test message");
        assert_eq!(entry.source, DiagnosticSource::Desktop);
    }
}
