use std::sync::atomic::{AtomicU8, Ordering};

const LEVEL_OFF: u8 = 0;
const LEVEL_ERROR: u8 = 1;
const LEVEL_WARN: u8 = 2;
const LEVEL_INFO: u8 = 3;
const LEVEL_DEBUG: u8 = 4;
const LEVEL_TRACE: u8 = 5;

static MAX_LEVEL: AtomicU8 = AtomicU8::new(LEVEL_OFF);

/// Sets the most verbose level that reaches the browser console. Logging starts off.
pub(crate) fn set_log_level(level: &str) -> Result<(), String> {
    let rank = match level {
        "off" => LEVEL_OFF,
        "error" => LEVEL_ERROR,
        "warn" => LEVEL_WARN,
        "info" => LEVEL_INFO,
        "debug" => LEVEL_DEBUG,
        "trace" => LEVEL_TRACE,
        _ => {
            return Err(format!(
                "invalid log level '{level}', expected off, error, warn, info, debug, or trace"
            ));
        }
    };
    MAX_LEVEL.store(rank, Ordering::Relaxed);
    tracing::callsite::rebuild_interest_cache();
    Ok(())
}

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
pub(crate) fn install() {
    let _ = tracing::subscriber::set_global_default(subscriber::ConsoleSubscriber);
}

#[cfg(any(test, all(target_arch = "wasm32", target_os = "unknown")))]
mod subscriber {
    use std::fmt::{self, Write as _};
    use std::sync::atomic::Ordering;

    use tracing::field::{Field, Visit};
    use tracing::level_filters::LevelFilter;
    use tracing::{Event, Level, Metadata, Subscriber, span};

    use super::{
        LEVEL_DEBUG, LEVEL_ERROR, LEVEL_INFO, LEVEL_OFF, LEVEL_TRACE, LEVEL_WARN, MAX_LEVEL,
    };

    fn level_rank(level: &Level) -> u8 {
        match *level {
            Level::ERROR => LEVEL_ERROR,
            Level::WARN => LEVEL_WARN,
            Level::INFO => LEVEL_INFO,
            Level::DEBUG => LEVEL_DEBUG,
            Level::TRACE => LEVEL_TRACE,
        }
    }

    fn rank_to_filter(rank: u8) -> LevelFilter {
        match rank {
            LEVEL_OFF => LevelFilter::OFF,
            LEVEL_ERROR => LevelFilter::ERROR,
            LEVEL_WARN => LevelFilter::WARN,
            LEVEL_INFO => LevelFilter::INFO,
            LEVEL_DEBUG => LevelFilter::DEBUG,
            _ => LevelFilter::TRACE,
        }
    }

    /// Minimal `tracing::Subscriber` that filters events by `MAX_LEVEL` and writes directly to
    /// `web_sys::console` without `tracing-subscriber` or `wasm-tracing`.
    pub(super) struct ConsoleSubscriber;

    impl Subscriber for ConsoleSubscriber {
        fn enabled(&self, metadata: &Metadata<'_>) -> bool {
            level_rank(metadata.level()) <= MAX_LEVEL.load(Ordering::Relaxed)
        }

        fn max_level_hint(&self) -> Option<LevelFilter> {
            Some(rank_to_filter(MAX_LEVEL.load(Ordering::Relaxed)))
        }

        fn new_span(&self, _span: &span::Attributes<'_>) -> span::Id {
            span::Id::from_u64(1)
        }

        fn record(&self, _span: &span::Id, _values: &span::Record<'_>) {}

        fn record_follows_from(&self, _span: &span::Id, _follows: &span::Id) {}

        fn event(&self, event: &Event<'_>) {
            if !self.enabled(event.metadata()) {
                return;
            }
            let line = format_event(event);
            emit_event(*event.metadata().level(), &line);
        }

        fn enter(&self, _span: &span::Id) {}

        fn exit(&self, _span: &span::Id) {}
    }

    fn emit_event(level: Level, line: &str) {
        #[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
        {
            let value = wasm_bindgen::JsValue::from_str(line);
            match level {
                Level::ERROR => web_sys::console::error_1(&value),
                Level::WARN => web_sys::console::warn_1(&value),
                Level::INFO => web_sys::console::info_1(&value),
                Level::DEBUG | Level::TRACE => web_sys::console::debug_1(&value),
            }
        }
        #[cfg(all(test, not(all(target_arch = "wasm32", target_os = "unknown"))))]
        super::tests::record_emitted(level, line);
    }

    #[derive(Default)]
    struct FieldVisitor {
        message: Option<String>,
        fields: String,
    }

    impl FieldVisitor {
        fn push_field(&mut self, name: &str, value: fmt::Arguments<'_>) {
            if !self.fields.is_empty() {
                self.fields.push(' ');
            }
            let _ = write!(self.fields, "{name}={value}");
        }
    }

    impl Visit for FieldVisitor {
        fn record_str(&mut self, field: &Field, value: &str) {
            if field.name() == "message" {
                self.message = Some(value.to_string());
            } else {
                self.push_field(field.name(), format_args!("{value}"));
            }
        }

        fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
            if field.name() == "message" {
                self.message = Some(format!("{value:?}"));
            } else {
                self.push_field(field.name(), format_args!("{value:?}"));
            }
        }
    }

    fn format_event(event: &Event<'_>) -> String {
        let meta = event.metadata();
        let mut visitor = FieldVisitor::default();
        event.record(&mut visitor);
        let mut line = String::new();
        let _ = write!(line, "[{} {}]", meta.level(), meta.target());
        if let Some(message) = visitor.message {
            line.push(' ');
            line.push_str(&message);
        }
        if !visitor.fields.is_empty() {
            line.push(' ');
            line.push_str(&visitor.fields);
        }
        line
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use tracing::Level;

    use super::subscriber::ConsoleSubscriber;
    use super::*;

    thread_local! {
        static RECORDED: RefCell<Vec<(Level, String)>> = const { RefCell::new(Vec::new()) };
    }

    pub(super) fn record_emitted(level: Level, line: &str) {
        RECORDED.with(|recorded| recorded.borrow_mut().push((level, line.to_string())));
    }

    fn take_recorded() -> Vec<(Level, String)> {
        RECORDED.with(|recorded| std::mem::take(&mut *recorded.borrow_mut()))
    }

    #[test]
    fn runtime_log_level_filtering_and_formatting() {
        let dispatch = tracing::Dispatch::new(ConsoleSubscriber);
        tracing::dispatcher::with_default(&dispatch, || {
            set_log_level("off").unwrap();
            let _ = take_recorded();

            tracing::error!(target: "lumen_wasm", code = 1, "hidden when off");
            assert!(take_recorded().is_empty());

            set_log_level("warn").unwrap();
            tracing::error!(target: "lumen_wasm", code = 42, "boom");
            tracing::warn!(target: "lumen_wasm", "heads up");
            tracing::info!(target: "lumen_wasm", "ignored at warn");
            let events = take_recorded();
            assert_eq!(events.len(), 2);
            assert_eq!(
                events[0],
                (Level::ERROR, "[ERROR lumen_wasm] boom code=42".to_string())
            );
            assert_eq!(
                events[1],
                (Level::WARN, "[WARN lumen_wasm] heads up".to_string())
            );

            set_log_level("trace").unwrap();
            tracing::debug!(target: "lumen_wasm", level = "trace", "set log level");
            tracing::trace!(target: "lumen_wasm", step = 3, "fine");
            let events = take_recorded();
            assert_eq!(events.len(), 2);
            assert_eq!(
                events[0],
                (
                    Level::DEBUG,
                    "[DEBUG lumen_wasm] set log level level=trace".to_string()
                )
            );
            assert_eq!(
                events[1],
                (Level::TRACE, "[TRACE lumen_wasm] fine step=3".to_string())
            );

            set_log_level("off").unwrap();
            tracing::error!(target: "lumen_wasm", "hidden again");
            assert!(take_recorded().is_empty());
        });

        let err = set_log_level("verbose").unwrap_err();
        assert!(err.contains("invalid log level 'verbose'"), "{err}");
    }
}
