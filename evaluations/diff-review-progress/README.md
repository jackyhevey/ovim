# Diff review and GUI navigation validation

Acceptance criteria:

- A file or reassigned section can be checked and hidden using a checkbox or `x`; `X` reveals it for unchecking. Both terminal layouts and GUI Files/Guided views work, including the final-item empty state.
- Review progress belongs to canonical changed lines. Switching layouts, removing an overlay, and reopening a review preserve checks within the editor session. Changing a file's patch invalidates its checks. Checking never changes source files, the index, or canonical patch coverage.
- Hidden items disappear from navigation and image exports. Context expansion retains correct source coordinates.
- `gd` uses the selected symbol's source position, including UTF-16 columns in GUI text. Stale saved lines and unknown source targets cannot issue a definition request against unrelated live text.
- Browser `:` retains focus while typing and after invalid commands. An old asynchronous command cannot dismiss a newer command line. Switching tabs dismisses browser command input without refocusing the old page.
- Ctrl+Tab and Ctrl+Shift+Tab follow visible workbench order, including browser inputs and pages with Vim keys disabled.

## Automated evidence

Validation on 2026-09-29:

- Full Rust headless workspace suite: 5,281 passed, 22 existing ignored tests. Subsequent core changes were rechecked with the complete diff review integration suite (56 passed) and native GUI suite (63 passed).
- GUI DOM suite: 178 passed; TypeScript checking and production build passed.
- Full Playwright suite: 136 passed across Chromium and WebKit. Run in `mcr.microsoft.com/playwright:v1.63.0-noble` because this Arch host lacks the Ubuntu ICU libraries required by the downloaded WebKit binary.
- Clippy covers all targets with both GUI and headless feature sets; Rust formatting and diff whitespace checks pass.
- Navigation routing test uses a subprocess JSON-RPC server with a project-scoped server ID, tests definition/implementation/type-definition LocationLinks, and rejects documents outside its root.
- `npm audit`: no reported vulnerabilities after updating the transitive undici dependency.

Reproduce the affected browser flows:

```sh
cd ovim/gui
npx playwright test e2e/review-navigation.spec.ts e2e/diff-viewer.spec.ts
```

The full browser run exposed an obsolete saved-review test: frozen reviews intentionally default to Guided, but the test immediately searched for a Files-only move-overlay button. It now verifies the Guided default and explicitly switches to Files before exercising move overlays.

## Manual flows

Used the real headless editor binary and its terminal renderer with a disposable Git repository. Opened the branch review, checked a file, verified only the remaining file was rendered, switched to split layout, revealed checked items, and confirmed the source bytes were unchanged. Captured renders exposed a split-header wrapping issue; the banner now reserves space for its checkbox annotation.

Used the installed rust-analyzer on a disposable Rust crate. Changed a call from a literal to `helper()`, opened the diff, positioned the terminal cursor inside the added call, and pressed `gd`. After indexing completed, navigation landed on `fn helper` at source line 1, column 4. Initial probes made before workspace indexing returned an empty definition response; the final check synchronizes on indexing/code-lens completion rather than treating the initialized transport as an indexed workspace.

Browser screenshots were inspected for regular and custom split views, including Japanese paths, Unicode source, review checkboxes, and toolbar controls. Browser tests exercise the actual Solid UI and injected browser keyboard script; the Tauri invoke boundary is mocked. Native projection/action tests exercise Rust separately. An interactive native desktop window was not available in this session.

Review checks are session-local. Saved custom arrangements remain restart-persistent through the existing review store; this change does not persist checkbox progress across process restarts.
