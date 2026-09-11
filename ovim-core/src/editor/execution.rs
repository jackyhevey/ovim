//! Boundaries for synchronous command execution and external effects.
//!
//! A scope batches clipboard synchronization, not edits or undo history.
//! Nested dispatch (mappings, macros, Ex commands) joins the outer scope.
//! Returning an error keeps completed edits and publishes their clipboard value.
//!
//! Physical input opens a scope per dispatch; a mapping RHS or macro runs inside
//! that dispatch. Explicit API SendKeys batches can open a wider scope. Direct
//! Ex entry points also open scopes, including when invoked without a frontend.
//! Merely collecting terminal events for rendering does not define a scope.
//!
//! Clipboard register names (+/*) select content, not synchronization timing.
//! Synchronous shell/Lua execution is an external boundary; asynchronous work and
//! queued interactive :! commands retain their existing scheduling. The scope
//! does not defer parsing, redraw policy, or undo recording.

use super::Editor;

impl Editor {
    /// Run synchronous commands as one unit of clipboard synchronization.
    /// Frontends may use this for an explicit programmatic command batch;
    /// unrelated physical events should remain separate scopes.
    pub fn with_execution_scope<R>(&mut self, run: impl FnOnce(&mut Self) -> R) -> R {
        let _scope = self.registers.execution_scope();
        run(self)
    }

    /// Publish clipboard effects before handing control to synchronous external
    /// code, then discard the external snapshot so subsequent reads see changes.
    /// This does not change scheduling of commands queued for a frontend.
    pub(crate) fn with_external_effects<R>(&mut self, run: impl FnOnce(&mut Self) -> R) -> R {
        let _boundary = self.registers.external_clipboard_scope();
        run(self)
    }
}
