# Provider permission selection

## Design and acceptance criteria

Claude Code owns permission execution. Ovim owns the user's selection and exposes
provider-supplied choices through the shared editor state to both interfaces.
There must be no frontend-maintained Claude mode catalog or separate preference
file. The existing model-selection document stores the permission selection;
documents without that field use the provider default (Auto for Claude Code).

The Claude Agent SDK 0.3.278 type contract and installed Claude Code 2.1.278 CLI
were checked against the official references on 2026-09-21:

- https://code.claude.com/docs/en/agent-sdk/permissions
- https://code.claude.com/docs/en/cli-reference

The canonical SDK IDs are `default`, `acceptEdits`, `plan`, `auto`, `dontAsk`,
and `bypassPermissions`. The CLI's `manual` is an alias for `default`.
Auto availability is enforced by Claude's installed runtime and account policy.
Plan requests planning behavior; Ovim Query independently restricts available
tools. Bypass requires the SDK's explicit `allowDangerouslySkipPermissions` flag.

Acceptance criteria:

1. Claude starts with Auto; both interfaces display the effective selection.
2. Every advertised mode reaches the runtime unchanged. Only Bypass enables the
   bypass flag. Query retains its tool and MCP restrictions for every mode.
3. A changed selection survives reopening/restarting via the existing preference
   document. Older documents remain readable. Invalid/stale selections cannot
   enable an unsupported mode or leak between providers.
4. Active turns reject changes before modifying live or persisted state.
5. GUI keyboard/mouse selection and terminal picker navigation use the core
   catalog. Unsupported providers have no permission control. Narrow layouts
   keep controls and selected options accessible.
6. Existing approval, question, model-selection, and cancellation flows continue
   to work; production assets contain the finished control.
7. Approval prompts show the exact command and its description, with the
   provider's reason when distinct. Other arguments remain reviewable as labeled
   text rather than a JSON envelope. Both interfaces use the same provider
   formatter; presentation never mutates the original approval payload.

## Validation boundary

Use core/editor and transport tests plus rendered Chromium scenarios. The prior
account-access restriction in `claude-code-validation.md` still applies: no live
Claude inference on this machine. Offline tests verify Ovim's contracts, not
Anthropic's account-backed permission decisions. Record actual outcomes below
before publishing.

## Verified outcomes (2026-09-22)

- `cargo test --workspace --locked`: 5,148 passed, zero failures, 22 existing
  ignored tests across 170 suites, including doctests.
- `cargo clippy --workspace --all-targets --locked -- -D warnings` and
  `cargo fmt --all --check`: passed.
- GUI TypeScript check and all 112 unit tests pass.
- All 16 offline Node adapter/MCP tests pass. Requests require an explicit mode,
  all six mode IDs are forwarded, Bypass alone enables the bypass flag, and
  Query keeps its restricted tool list even with Bypass selected.
- All 46 Playwright scenarios pass in Chromium and WebKit. Permission scenarios
  use 14 profiles at 1154×1054 and 900×700, exercise keyboard and mouse changes,
  project the selection back through the bridge, reopen on Bypass, and switch
  to an unsupported provider. Screenshots under `ovim/gui/test-results/` show
  the picker and command/description approval card.
- Rendered checks found and fixed two picker defects: the selected last row was
  hidden when reopening, and a crowded picker overflowed a short viewport.
  The selected row now scrolls into view and height uses the trigger's actual
  position.
- Full WebKit checks exposed proportional font substitution for unavailable
  named editor fonts on this host. The existing cell-measurement owner now
  detects unequal M/i advances and falls back to generic monospace, reconsidering
  the configured font on font-load events. The text-placement scenarios also
  exercise deliberate proportional substitution. Segment-start tolerance stays
  at 0.15px and glyph tolerance below 1px; glyph centers avoid WebKit's separately
  rounded Range edges at fractional sizes.
- Real terminal smoke check used a disposable config, run store, and preference
  file. An older document selecting Claude/Opus opened with Auto. `/permissions`
  opened the picker; `/permissions plan` and arrow navigation updated that same
  document. A second Ovim process restored Opus and Don't ask. At 80×22 the
  selected Bypass row remained visible; Shift-Tab then Down changed effort to
  Low while preserving Bypass. Captures and the exact JSON are retained in
  `/tmp/ovim-permissions-qa-0poycvg4/`. No messages were submitted to Claude.
- The Rust completion regression exposed an unsupported-provider menu entry.
  `/permissions` completion now follows the provider capability, preserving the
  existing command menu for other providers. Added keyboard section navigation
  and short-panel mouse-target tests.
