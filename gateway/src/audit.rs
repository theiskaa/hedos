//! The audit record for a served request and the sink that stores it. A no-op
//! sink is provided; a disk-backed JSONL log implements the same trait later.

/// One line of the audit log: who called, what they asked for, and how it went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatewayAuditEntry {
    /// When the request completed, in epoch milliseconds.
    pub ts_millis: i64,
    /// The client's id, if authenticated.
    pub client: Option<String>,
    /// The client's display name, if authenticated.
    pub client_name: Option<String>,
    /// The HTTP method.
    pub method: String,
    /// The request path.
    pub route: String,
    /// The model served, if any.
    pub model: Option<String>,
    /// The capability exercised, if any.
    pub capability: Option<String>,
    /// The audit outcome label.
    pub outcome: String,
    /// The HTTP status returned.
    pub status: u16,
    /// How long the request took, in milliseconds.
    pub duration_ms: i64,
    /// Extra detail (e.g. an internal error description).
    pub detail: Option<String>,
}

/// A sink for audit entries.
pub trait Auditing: Send + Sync {
    /// Record a served request.
    fn append(&self, entry: GatewayAuditEntry);

    /// Record a rejected unauthenticated request. Defaults to [`append`](Self::append);
    /// a real log may rate-limit these.
    fn append_unauthorized(&self, entry: GatewayAuditEntry) {
        self.append(entry);
    }
}

/// An audit sink that discards everything.
pub struct NoopAudit;

impl Auditing for NoopAudit {
    fn append(&self, _entry: GatewayAuditEntry) {}
}
