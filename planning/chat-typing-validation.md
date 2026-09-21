# Chat typing scroll regression

Typing into an eight-line GUI draft reproduced a WebKit transcript jump from
scrollTop 1411 to 1315 while the final transcript height stayed at 496 pixels.
The composer reset the live textarea height to `auto` before reading scrollHeight.
That intermediate layout enlarged the transcript viewport and clamped its scroll
position. A subsequent snapshot could scroll back to the bottom.

The composer now measures an invisible, out-of-flow textarea copy in the same
styling context, then updates the visible input directly to its final height.
The transcript's existing scrolling policy is unchanged.

Acceptance criteria and reusable coverage: `ovim/gui/e2e/chat-typing.spec.ts`.
Use a scrollable 20-message conversation with 1-, 8-, and 30-line drafts. Type
individual characters at the bottom and while reading older messages. Transcript
scroll position and viewport height must remain stable when draft height is
unchanged. Real content changes must still grow the input, cap it at 220 pixels,
and shrink it again. Screenshots are emitted under GUI `test-results/`.

Validation after rebasing onto f50451c0:

- TypeScript check, all 112 GUI unit tests, and production build passed.
- All 52 browser tests passed across Chromium and WebKit, including six new typing cases.
- Inspected the rendered WebKit multiline-draft screenshot.
- Initial full-suite testing exposed font-layout failures; f50451c0 contains their
  correction, now included and verified on this branch.

Browser checks use the actual GUI components and deterministic mock projections.
No native desktop binary or live AI provider was exercised; the reproduced defect
is in browser layout during local input, before provider involvement.
