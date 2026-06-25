#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RiskLevel {
    Low,
    Medium,
    High,
}

/// Describes a tool action that requires explicit user approval before execution.
/// Pure data — no methods, no logic. Owned by the runtime between proposal and resolution.
#[derive(Debug, Clone)]
pub struct PendingAction {
    pub tool_name: String,
    pub summary: String,
    pub risk: RiskLevel,
    /// Whether the action can be undone after execution. File edits, writes, and git
    /// branch/commit operations are reversible; MCP tools are classified heuristically.
    /// Irreversible actions are never grouped into a transaction and surface a stronger
    /// confirmation in the TUI. thunk classifies this — MCP server declarations are ignored.
    pub reversible: bool,
    /// Opaque serialized payload passed back to the tool's execute_approved().
    pub payload: String,
}

/// A group of one or more pending actions presented to the user as a single approval.
/// Single-action wrapping preserves backward compatibility with the existing approval path.
#[derive(Debug, Clone)]
pub struct PendingTransaction {
    pub actions: Vec<PendingAction>,
}

impl PendingTransaction {
    pub fn single(action: PendingAction) -> Self {
        Self {
            actions: vec![action],
        }
    }

    pub fn is_single(&self) -> bool {
        self.actions.len() == 1
    }

    pub fn first(&self) -> &PendingAction {
        &self.actions[0]
    }

    /// Consume a single-action transaction into its one action.
    /// Panics in debug if the transaction has more than one action.
    pub fn into_single(self) -> PendingAction {
        debug_assert!(
            self.is_single(),
            "into_single called on multi-action transaction"
        );
        self.actions.into_iter().next().unwrap()
    }
}

/// Tracks which phase of the approval lifecycle a pending transaction is in.
///
/// `AwaitingPreCheck` — freshly proposed; pre-edit LSP check has not run yet.
/// `PreCheckComplete` — pre-check ran (or was bypassed); safe to execute immediately.
#[derive(Debug)]
pub enum PendingApprovalStage {
    AwaitingPreCheck(PendingTransaction),
    PreCheckComplete(PendingTransaction),
}

impl PendingApprovalStage {
    /// Consumes the stage and returns the full transaction.
    pub fn into_transaction(self) -> PendingTransaction {
        match self {
            Self::AwaitingPreCheck(tx) | Self::PreCheckComplete(tx) => tx,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_action_is_cloneable() {
        let action = PendingAction {
            tool_name: "edit_file".to_string(),
            summary: "Edit src/lib.rs (3 lines)".to_string(),
            risk: RiskLevel::Low,
            reversible: true,
            payload: "{}".to_string(),
        };
        let cloned = action.clone();
        assert_eq!(cloned.tool_name, "edit_file");
        assert_eq!(cloned.risk, RiskLevel::Low);
        assert_eq!(cloned.summary, action.summary);
    }

    #[test]
    fn risk_levels_are_comparable() {
        assert_eq!(RiskLevel::Low, RiskLevel::Low);
        assert_ne!(RiskLevel::Low, RiskLevel::High);
        assert_ne!(RiskLevel::Medium, RiskLevel::High);
    }

    #[test]
    fn pending_transaction_single_wraps_one_action() {
        let action = PendingAction {
            tool_name: "edit_file".to_string(),
            summary: "edit a.rs".to_string(),
            risk: RiskLevel::Medium,
            reversible: true,
            payload: "payload".to_string(),
        };
        let tx = PendingTransaction::single(action.clone());
        assert!(tx.is_single());
        assert_eq!(tx.first().tool_name, "edit_file");
        assert_eq!(tx.into_single().summary, "edit a.rs");
    }

    #[test]
    fn pending_transaction_multi_is_not_single() {
        let make = |name: &str| PendingAction {
            tool_name: name.to_string(),
            summary: name.to_string(),
            risk: RiskLevel::Medium,
            reversible: true,
            payload: String::new(),
        };
        let tx = PendingTransaction {
            actions: vec![make("edit_file"), make("write_file")],
        };
        assert!(!tx.is_single());
        assert_eq!(tx.first().tool_name, "edit_file");
    }

    #[test]
    fn stage_into_transaction_returns_full_tx() {
        let action = PendingAction {
            tool_name: "write_file".to_string(),
            summary: "write b.rs".to_string(),
            risk: RiskLevel::Low,
            reversible: true,
            payload: String::new(),
        };
        let stage = PendingApprovalStage::AwaitingPreCheck(PendingTransaction::single(action));
        let tx = stage.into_transaction();
        assert_eq!(tx.actions.len(), 1);
        assert_eq!(tx.first().tool_name, "write_file");
    }
}
