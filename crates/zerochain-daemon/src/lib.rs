//! MCP server daemon for zerochain workflow execution.

pub mod cli;
pub mod mcp;

// Re-export primary public API types from the engine crate so existing
// consumers don't break immediately.
pub use zerochain_engine::{AppState, DaemonError, InitWorkflowParams, InitWorkflowRequest};

/// Status marker for one stage in human-readable status listings.
///
/// A stage carrying both `.error` and `.complete` markers is an errored stage:
/// this matches the execution plan's precedence, which treats `is_error` as
/// blocking regardless of completion.
pub fn stage_marker(is_error: bool, is_complete: bool, human_gate: bool) -> &'static str {
    if is_error {
        "error"
    } else if is_complete {
        "done"
    } else if human_gate {
        "gate"
    } else {
        "pending"
    }
}

#[cfg(test)]
mod tests {
    use super::stage_marker;

    #[test]
    fn error_takes_precedence_over_complete() {
        // A stage with both markers (e.g. reject-after-approve) must display
        // as errored, matching the execution plan.
        assert_eq!(stage_marker(true, true, false), "error");
        assert_eq!(stage_marker(true, false, false), "error");
    }

    #[test]
    fn complete_gate_pending_fallbacks() {
        assert_eq!(stage_marker(false, true, false), "done");
        assert_eq!(stage_marker(false, true, true), "done");
        assert_eq!(stage_marker(false, false, true), "gate");
        assert_eq!(stage_marker(false, false, false), "pending");
    }
}
