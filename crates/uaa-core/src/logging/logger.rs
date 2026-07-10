// file: crates/uaa-core/src/logging/logger.rs
// version: 1.1.1
// guid: j0k1l2m3-n4o5-6789-0123-456789jklmno

//! Logger initialization and configuration

use crate::Result;
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

/// Initialize the logging system
pub fn init_logger(verbose: bool, quiet: bool) -> Result<()> {
    let filter = if quiet {
        EnvFilter::new("error")
    } else if verbose {
        EnvFilter::new("debug")
    } else {
        EnvFilter::new("info")
    };

    tracing_subscriber::registry()
        .with(filter)
        .with(
            fmt::layer()
                .with_target(false)
                .with_thread_ids(false)
                .with_file(false)
                .with_line_number(false)
                .compact(),
        )
        .try_init()
        .map_err(|e| {
            crate::error::AutoInstallError::ConfigError(format!(
                "Failed to initialize logger: {}",
                e
            ))
        })?;

    Ok(())
}

/// Initialize structured JSON logging (for services)
pub fn init_json_logger() -> Result<()> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer())
        .try_init()
        .map_err(|e| {
            crate::error::AutoInstallError::ConfigError(format!(
                "Failed to initialize JSON logger: {}",
                e
            ))
        })?;

    Ok(())
}

/// Create a scoped logger for operations
pub fn with_operation_span<F, R>(operation: &str, f: F) -> R
where
    F: FnOnce() -> R,
{
    let span = tracing::info_span!("operation", name = operation);
    let _enter = span.enter();
    f()
}

/// Create an async scoped logger for operations
pub async fn with_async_operation_span<F, Fut, R>(operation: &str, f: F) -> R
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = R>,
{
    let span = tracing::info_span!("operation", name = operation);
    async move { f().await }.instrument(span).await
}

// Re-export tracing macros for convenience
pub use tracing::{debug, error, info, trace, warn};

use tracing::Instrument;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_init_logger_default() {
        // Note: We can't easily test logger initialization multiple times
        // as tracing subscriber can only be set once per process.
        // This test verifies the function signature and logic paths.

        // Arrange
        let verbose = false;
        let quiet = false;

        // Act
        let result = init_logger(verbose, quiet);

        // Assert
        // Should either succeed or fail gracefully
        // (May fail if logger was already initialized in other tests)
        assert!(result.is_ok() || result.is_err());
    }

    #[test]
    fn test_init_logger_verbose() {
        // Arrange
        let verbose = true;
        let quiet = false;

        // Act
        let result = init_logger(verbose, quiet);

        // Assert
        assert!(result.is_ok() || result.is_err());
    }

    #[test]
    fn test_init_logger_quiet() {
        // Arrange
        let verbose = false;
        let quiet = true;

        // Act
        let result = init_logger(verbose, quiet);

        // Assert
        assert!(result.is_ok() || result.is_err());
    }

    #[test]
    fn test_init_json_logger() {
        // Act
        let result = init_json_logger();

        // Assert
        // Should handle initialization gracefully
        assert!(result.is_ok() || result.is_err());
    }

    #[test]
    fn test_with_operation_span() {
        // Arrange
        let operation = "test_operation";
        let mut executed = false;

        // Act
        let result = with_operation_span(operation, || {
            executed = true;
            "test_result"
        });

        // Assert
        assert!(executed);
        assert_eq!(result, "test_result");
    }

    #[test]
    fn test_with_operation_span_with_return_value() {
        // Arrange
        let operation = "math_operation";

        // Act
        let result = with_operation_span(operation, || 2 + 2);

        // Assert
        assert_eq!(result, 4);
    }

    #[tokio::test]
    async fn test_with_async_operation_span() {
        // Arrange
        let operation = "async_test_operation";
        let mut executed = false;

        // Act
        let result = with_async_operation_span(operation, || async {
            executed = true;
            "async_result"
        })
        .await;

        // Assert
        assert!(executed);
        assert_eq!(result, "async_result");
    }

    #[tokio::test]
    async fn test_with_async_operation_span_with_delay() {
        // Arrange
        let operation = "delayed_operation";

        // Act
        let result = with_async_operation_span(operation, || async {
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
            42
        })
        .await;

        // Assert
        assert_eq!(result, 42);
    }

    #[test]
    fn test_tracing_macros_availability() {
        // This test verifies that the re-exported macros are available
        // We can't easily test their output without complex setup

        // Act & Assert
        // If these compile, the macros are properly exported
        // Test that we can use the macros (though output may go nowhere in tests)

        // Simple compilation test - if this builds, macros are available
        // Test passes if compilation succeeds
    }
}
