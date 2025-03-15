#[cfg(test)]
use serde_json::value::Value;
use slog_extlog::slog_test;

/// A logger that captures log messages for testing.
pub struct CapturingLogger {
    pub(crate) logger: slog::Logger,
    buffer: iobuffer::IoBuffer,
}

impl CapturingLogger {
    /// Create a new `CapturingLogger`.
    pub fn new() -> Self {
        let buffer = iobuffer::IoBuffer::new();
        let logger = slog_test::new_test_logger(buffer.clone());
        Self { logger, buffer }
    }

    /// Get the logs that have been captured.
    pub fn logs(&mut self) -> Vec<Value> {
        slog_test::read_json_values(&mut self.buffer)
    }

    /// Get log messages that would usually be visible when git-absorb is run.
    ///
    /// Used to filter out debug logs which are too detailed for most tests.
    pub fn visible_logs(&mut self) -> Vec<Value> {
        // let logs = slog_test::read_json_values(&mut self.buffer);
        let logs = self.logs();
        logs.iter()
            .filter(|log| log["level"].as_str().unwrap().ne("DEBG"))
            .map(|log| log.clone())
            .collect()
    }
}

/// Assert that the actual log messages match the expected log messages.
///
/// There must be the same number of items in `actual_logs` and `expected_logs`.
/// The items are compared in order.
/// Elements in `actual_logs` items that do not appear in `expected_logs` are ignored.
pub fn assert_log_messages_are(actual_logs: Vec<Value>, expected_logs: Vec<&Value>) {
    assert_eq!(
        actual_logs.len(),
        expected_logs.len(),
        "Log lengths do not match. Found:\n{:?}",
        actual_logs,
    );

    for (actual, expected) in actual_logs.iter().zip(expected_logs.iter()) {
        slog_test::assert_json_matches(actual, expected);
    }
}
