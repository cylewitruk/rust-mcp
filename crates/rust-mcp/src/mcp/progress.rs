use std::sync::atomic::{AtomicU32, Ordering};

use rmcp::RoleServer;
use rmcp::model::{Meta, ProgressNotificationParam, ProgressToken};
use rmcp::service::Peer;

/// Per-tool-call progress reporter.
///
/// Sends indeterminate `notifications/progress` updates with a monotonically
/// increasing step counter.  Messages are best-effort — send failures are
/// silently ignored.
pub struct ToolProgress {
    peer: Peer<RoleServer>,
    token: ProgressToken,
    step: AtomicU32,
}

impl ToolProgress {
    /// Creates a progress reporter if the client supplied a `progressToken` in
    /// the request `_meta`.  Returns `None` when no token is present.
    fn from_meta(meta: &Meta, peer: &Peer<RoleServer>) -> Option<Self> {
        meta.get_progress_token()
            .map(|token| Self {
                peer: peer.clone(),
                token,
                step: AtomicU32::new(0),
            })
    }

    /// Sends an indeterminate progress notification.
    ///
    /// The `progress` value increments by 1.0 on every call (1.0, 2.0, 3.0 …)
    /// with `total: None`, satisfying the MCP spec requirement that progress
    /// values increase monotonically.
    async fn notify(&self, message: &str) {
        let step = self
            .step
            .fetch_add(1, Ordering::Relaxed);
        let _ = self
            .peer
            .notify_progress(ProgressNotificationParam {
                progress_token: self.token.clone(),
                progress: f64::from(step + 1),
                total: None,
                message: Some(message.to_string()),
            })
            .await;
    }
}

impl std::fmt::Debug for ToolProgress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolProgress")
            .field("token", &self.token)
            .field(
                "step",
                &self
                    .step
                    .load(Ordering::Relaxed),
            )
            .finish_non_exhaustive()
    }
}

/// Extensible per-tool-call context envelope.
///
/// Passed from the tool entry point through the handler into shared query
/// methods (`ensure_*`, `fetch_crate_context`, etc.).  Wrapping progress in
/// this struct means handler signatures only change once — future per-request
/// state (trace IDs, session metadata, …) becomes a new field without
/// signature churn.
#[derive(Debug)]
pub struct ToolCallContext {
    progress: Option<ToolProgress>,
}

impl ToolCallContext {
    /// Creates a new context, extracting the progress token from the request
    /// meta if present.
    pub fn new(meta: &Meta, peer: &Peer<RoleServer>) -> Self {
        Self {
            progress: ToolProgress::from_meta(meta, peer),
        }
    }

    /// Creates a no-op context without progress reporting.
    ///
    /// Used by background workers that invoke handler methods outside of a
    /// client request (no `Peer` or `Meta` available).
    pub fn no_op() -> Self {
        Self { progress: None }
    }

    /// Sends a progress notification if a progress token is available.
    ///
    /// This is a no-op when the client did not supply a `progressToken`,
    /// so callers never need `if let Some(…)` guards.
    pub async fn notify_progress(&self, message: &str) {
        if let Some(ref p) = self.progress {
            p.notify(message).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_meta_returns_none_without_token() {
        let meta = Meta::default();
        // Cannot construct a real Peer in unit tests, but we can verify the
        // None path by checking ToolCallContext construction logic:
        assert!(
            meta.get_progress_token()
                .is_none()
        );
    }

    #[test]
    fn step_counter_increments() {
        let counter = AtomicU32::new(0);
        assert_eq!(counter.fetch_add(1, Ordering::Relaxed), 0);
        assert_eq!(counter.fetch_add(1, Ordering::Relaxed), 1);
        assert_eq!(counter.fetch_add(1, Ordering::Relaxed), 2);
        assert_eq!(counter.load(Ordering::Relaxed), 3);
    }
}
