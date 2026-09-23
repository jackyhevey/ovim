import { readFile } from "node:fs/promises";
import { unzipSync } from "fflate";
import { expect, test, type Page } from "@playwright/test";
import type { GuiDiffDocument, GuiDiffLine } from "../src/types";

const context = (
    count: number,
    oldStart: number,
    newStart: number,
): GuiDiffLine[] =>
    Array.from({ length: count }, (_, index) => ({
        kind: "context",
        text: `    processRequest(request, ${index});`,
        oldLine: oldStart + index,
        newLine: newStart + index,
        reviewLine: oldStart + index,
    }));

const review: GuiDiffDocument = {
    title: "Diff · main → payment-api",
    layout: "split",
    managed: true,
    custom: false,
    files: [
        {
            id: "src/payments/PaymentResource.java",
            path: "src/payments/PaymentResource.java",
            status: "modified",
            additions: 5,
            deletions: 24,
            binary: false,
            metadata: [],
            hunks: [
                {
                    header: "@@ -1200,104 +1200,85 @@ public Response acceptPayment()",
                    oldStart: 1200,
                    oldCount: 104,
                    newStart: 1200,
                    newCount: 85,
                    reviewLine: 2,
                    lines: [
                        ...context(4, 1200, 1200),
                        ...Array.from(
                            { length: 24 },
                            (_, index): GuiDiffLine => ({
                                kind: "removed",
                                text: `    paymentService.validateLegacyPayment(request, ${index});`,
                                oldLine: 1204 + index,
                                reviewLine: 6 + index,
                            }),
                        ),
                        ...Array.from(
                            { length: 5 },
                            (_, index): GuiDiffLine => ({
                                kind: "added",
                                text: `    paymentService.acceptPayment(request, ${index});`,
                                newLine: 1204 + index,
                                reviewLine: 30 + index,
                            }),
                        ),
                        ...context(76, 1228, 1209),
                    ],
                },
            ],
        },
        {
            id: "assets/receipt.png",
            path: "assets/receipt.png",
            status: "modified",
            additions: 0,
            deletions: 0,
            binary: true,
            metadata: [],
            hunks: [],
        },
    ],
};

async function setup(page: Page, document: GuiDiffDocument = review) {
    await page.route("**/src/mock.ts", async (route) => {
        const response = await route.fetch();
        await route.fulfill({
            response,
            body: `${await response.text()}
                delete mockSnapshot.aiChat;
                delete mockSnapshot.fileTree;
                mockSnapshot.fileName = ${JSON.stringify(document.title)};
                mockSnapshot.filePath = undefined;
                mockSnapshot.readOnly = true;
                mockSnapshot.panes = [{...mockSnapshot.panes[0], diffReview: ${JSON.stringify(document)}}];`,
        });
    });
    await page.goto("/");
    await expect(page.locator(".flow-diff")).toBeVisible();
}

const movedReview: GuiDiffDocument = {
    title: "Diff · Extract parser",
    layout: "split",
    managed: true,
    custom: true,
    files: [
        {
            id: "src/parser.ts",
            path: "src/parser.ts",
            status: "modified",
            additions: 3,
            deletions: 0,
            binary: false,
            metadata: [],
            hunks: [
                {
                    header: "@@ -1,4 +1,7 @@",
                    oldStart: 1,
                    oldCount: 4,
                    newStart: 1,
                    newCount: 7,
                    lines: [
                        {
                            kind: "added",
                            text: 'import { parseTokens } from "./tokens";',
                            newLine: 1,
                        },
                        {
                            kind: "context",
                            text: "// Parser helpers",
                            oldLine: 1,
                            newLine: 2,
                        },
                        { kind: "context", text: "", oldLine: 2, newLine: 3 },
                        {
                            kind: "added",
                            text: "export function parse(input: string) {",
                            newLine: 4,
                        },
                        {
                            kind: "added",
                            text: "  return parseTokens(input); }",
                            newLine: 5,
                        },
                        { kind: "context", text: "", oldLine: 3, newLine: 6 },
                        {
                            kind: "context",
                            text: "export const version = 2;",
                            oldLine: 4,
                            newLine: 7,
                        },
                    ],
                },
            ],
        },
        {
            id: "src/main.ts",
            path: "src/main.ts",
            status: "modified",
            additions: 0,
            deletions: 3,
            binary: false,
            metadata: [],
            hunks: [
                {
                    header: "@@ -42,7 +42,4 @@",
                    oldStart: 42,
                    oldCount: 7,
                    newStart: 42,
                    newCount: 4,
                    lines: [
                        ...context(2, 42, 42),
                        {
                            kind: "removed",
                            text: "function parse(input: string) {",
                            oldLine: 44,
                        },
                        {
                            kind: "removed",
                            text: "  return legacyParse(input);",
                            oldLine: 45,
                        },
                        { kind: "removed", text: "}", oldLine: 46 },
                        ...context(2, 47, 44),
                    ],
                },
            ],
        },
    ],
    guidedFiles: [
        {
            id: "pair_0",
            label: "Parser moved and simplified",
            path: "src/parser.ts",
            oldPath: "src/main.ts",
            status: "reassigned",
            additions: 2,
            deletions: 3,
            binary: false,
            metadata: [],
            hunks: [
                {
                    header: "Parser moved and simplified",
                    oldStart: 44,
                    oldCount: 3,
                    newStart: 4,
                    newCount: 2,
                    lines: [
                        {
                            kind: "removed",
                            text: "function parse(input: string) {",
                            oldLine: 44,
                        },
                        {
                            kind: "removed",
                            text: "  return legacyParse(input);",
                            oldLine: 45,
                        },
                        { kind: "removed", text: "}", oldLine: 46 },
                        {
                            kind: "added",
                            text: "export function parse(input: string) {",
                            newLine: 4,
                        },
                        {
                            kind: "added",
                            text: "  return parseTokens(input); }",
                            newLine: 5,
                        },
                    ],
                },
            ],
        },
        {
            id: "residual_0",
            path: "src/parser.ts",
            status: "modified",
            additions: 1,
            deletions: 0,
            binary: false,
            metadata: [],
            hunks: [
                {
                    header: "Remaining changes",
                    oldStart: 1,
                    oldCount: 0,
                    newStart: 1,
                    newCount: 1,
                    lines: [
                        {
                            kind: "added",
                            text: 'import { parseTokens } from "./tokens";',
                            newLine: 1,
                        },
                    ],
                },
            ],
        },
    ],
    moves: [
        {
            id: "pair_0",
            label: "Parser moved and simplified",
            old: {
                path: "src/main.ts",
                startLine: 44,
                lineCount: 3,
                contextComplete: true,
                contextWindows: [
                    {
                        startLine: 1,
                        lines: Array.from({ length: 90 }, (_, index) => ({
                            kind:
                                index >= 43 && index <= 45
                                    ? ("removed" as const)
                                    : ("context" as const),
                            oldLine: index + 1,
                            text:
                                index === 43
                                    ? "function parse(input: string) {"
                                    : index === 44
                                      ? "  return legacyParse(input);"
                                      : index === 45
                                        ? "}"
                                        : `// Main source context ${index + 1}`,
                        })),
                    },
                ],
            },
            new: {
                path: "src/parser.ts",
                startLine: 4,
                lineCount: 2,
                contextComplete: true,
                contextWindows: [
                    {
                        startLine: 1,
                        lines: Array.from({ length: 60 }, (_, index) => ({
                            kind:
                                index === 3 || index === 4
                                    ? ("added" as const)
                                    : ("context" as const),
                            newLine: index + 1,
                            text:
                                index === 3
                                    ? "export function parse(input: string) {"
                                    : index === 4
                                      ? "  return parseTokens(input); }"
                                      : `// Parser source context ${index + 1}`,
                        })),
                    },
                ],
            },
        },
    ],
};

test("moved segments stay with file changes, scroll independently and switch reconstruction side", async ({
    page,
}) => {
    const errors: string[] = [];
    page.on("pageerror", (error) => errors.push(error.message));
    await setup(page, movedReview);
    const picker = page.getByRole("combobox", { name: "Changed file" });
    await expect(picker.locator("option")).toHaveCount(2);
    await expect(
        page
            .locator(".flow-scroll.new")
            .getByText('import { parseTokens } from "./tokens";', {
                exact: true,
            }),
    ).toBeVisible();
    const overlay = page.getByRole("complementary", {
        name: "Moved segment: Parser moved and simplified",
    });
    await expect(
        page.locator(".flow-scroll.old").locator(".flow-move-overlay"),
    ).toBeVisible();
    await expect(
        overlay.getByText("src/main.ts", { exact: true }),
    ).toBeVisible();
    const scroller = overlay.locator(".flow-move-scroll");
    await expect
        .poll(() => scroller.evaluate((element) => element.scrollTop))
        .toBeGreaterThan(0);
    await page.screenshot({
        path: test.info().outputPath("custom-diff-overlay-initial.png"),
    });
    const outerTop = await page
        .locator(".flow-scroll.old")
        .evaluate((element) => element.scrollTop);
    await scroller.focus();
    await page.keyboard.press("k");
    await scroller.evaluate((element) => {
        element.scrollTop = 0;
    });
    await expect(
        overlay.getByText("// Main source context 1", { exact: true }),
    ).toBeVisible();
    expect(
        await page
            .locator(".flow-scroll.old")
            .evaluate((element) => element.scrollTop),
    ).toBe(outerTop);
    await page.screenshot({
        path: test.info().outputPath("custom-diff-overlay-old.png"),
    });
    await scroller.focus();
    await page.keyboard.press("n");
    await page.getByRole("button", { name: "After", exact: true }).click();
    await expect(picker).toHaveValue("src/main.ts");
    await expect(
        page.locator(".flow-scroll.new .flow-move-overlay"),
    ).toBeVisible();
    await expect(
        overlay.getByText("src/parser.ts", { exact: true }),
    ).toBeVisible();
    await page.screenshot({
        path: test.info().outputPath("custom-diff-overlay-new.png"),
    });
    await page.setViewportSize({ width: 760, height: 720 });
    const fits = await page
        .locator(".flow-diff")
        .evaluate(
            (element) =>
                element.getBoundingClientRect().right <= window.innerWidth,
        );
    expect(fits).toBe(true);
    const overlayFits = await overlay.evaluate((element) => {
        const pane = element.closest(".flow-scroll")!.getBoundingClientRect();
        const card = element.getBoundingClientRect();
        return card.left >= pane.left && card.right <= pane.right;
    });
    expect(overlayFits).toBe(true);
    expect(
        await picker.evaluate(
            (element) => element.getBoundingClientRect().width,
        ),
    ).toBeGreaterThanOrEqual(140);
    await expect(overlay.locator(".flow-move-heading")).toHaveAttribute(
        "title",
        /Parser moved and simplified/,
    );
    await page.screenshot({
        path: test.info().outputPath("custom-diff-overlay-narrow.png"),
    });
    expect(errors).toEqual([]);
});

test("guided view keeps explanations visible and supports stepping through sections", async ({
    page,
}) => {
    await setup(page, movedReview);
    await page.getByRole("button", { name: "Guided", exact: true }).click();
    const picker = page.getByRole("combobox", { name: "Guided section" });
    await expect(picker).toHaveValue("pair_0");
    await expect(page.locator(".flow-file-heading strong")).toContainText(
        "Parser moved and simplified",
    );
    await expect(
        page
            .locator(".flow-scroll.old")
            .getByText("  return legacyParse(input);", { exact: true }),
    ).toBeVisible();
    // Clicking Guided leaves a native button focused; review shortcuts still work.
    await page.keyboard.press("n");
    await expect(picker).toHaveValue("residual_0");
    await page.keyboard.press("N");
    await expect(picker).toHaveValue("pair_0");
    await page.screenshot({
        path: test.info().outputPath("custom-diff-guided.png"),
    });
    await page.getByRole("button", { name: "Files", exact: true }).click();
    await expect(
        page.getByRole("combobox", { name: "Changed file" }),
    ).toHaveValue("src/parser.ts");
    await expect(page.locator(".flow-move-overlay")).toBeVisible();
});

test("stale restructuring can be recovered through native diff actions", async ({
    page,
}) => {
    await page.route(
        "**/node_modules/.vite/deps/@tauri-apps_api_event.js*",
        (route) =>
            route.fulfill({
                contentType: "application/javascript",
                body: "export const listen = async () => () => {};",
            }),
    );
    await page.route(
        "**/node_modules/.vite/deps/@tauri-apps_api_core.js*",
        (route) =>
            route.fulfill({
                contentType: "application/javascript",
                body: `
            import { mockSnapshot } from '/src/mock.ts';
            export const isTauri = () => true;
            export class Channel {}
            window.diffCommands = [];
            let listener;
            let current = mockSnapshot;
            const savedDocument = ${JSON.stringify(movedReview)};
            export const invoke = async (command, args) => {
                window.diffCommands.push({command, args});
                if (command === 'gui_subscribe') {
                    listener = args.onEvent;
                    listener.onmessage(mockSnapshot);
                }
                if (command === 'gui_diff_action') {
                    const document = current.panes[0].diffReview;
                    const mode = args.action === 'open_saved_overlay' ? 'saved'
                        : args.action === 'return_to_live_diff' ? 'stale'
                        : document.overlay.mode === 'active' ? 'available' : 'active';
                    const nextDocument = mode === 'saved' ? savedDocument
                        : {...savedDocument, custom: false, moves: [], guidedFiles: []};
                    current = {...current, revision: current.revision + 1,
                        panes: [{...current.panes[0], diffReview: {...nextDocument,
                            overlay: {...document.overlay, mode}}}]};
                    listener.onmessage(current);
                }
            };`,
            }),
    );
    await setup(page, {
        ...movedReview,
        custom: false,
        moves: [],
        guidedFiles: [],
        overlay: { mode: "stale", title: "Extract parser" },
    });
    await expect(
        page.getByText("Saved restructuring no longer matches these changes."),
    ).toBeVisible();
    await page.screenshot({
        path: test.info().outputPath("custom-diff-stale.png"),
    });
    await expect(page.locator(".flow-move-overlay")).toHaveCount(0);
    await page.getByRole("button", { name: "View saved review" }).click();
    await expect(page.locator(".flow-move-overlay")).toBeVisible();
    await expect(
        page.getByRole("button", { name: "Return to live diff" }),
    ).toBeVisible();
    await page.getByRole("button", { name: "Return to live diff" }).click();
    await expect(
        page.getByRole("button", { name: "View saved review" }),
    ).toBeVisible();
    await expect(page.locator(".flow-move-overlay")).toHaveCount(0);
    const actions = await page.evaluate(() =>
        (window as any).diffCommands
            .filter((item: any) => item.command === "gui_diff_action")
            .map((item: any) => item.args),
    );
    expect(actions).toEqual([
        expect.objectContaining({
            pane: 0,
            bufferId: expect.any(Number),
            action: "open_saved_overlay",
        }),
        expect.objectContaining({
            pane: 0,
            bufferId: expect.any(Number),
            action: "return_to_live_diff",
        }),
    ]);
});

test("compact diff panes stay centered and draw unequal change connectors", async ({
    page,
}) => {
    const errors: string[] = [];
    page.on("pageerror", (error) => errors.push(error.message));
    await setup(page);
    await expect(page.locator(".flow-diff svg path").first()).toBeVisible();
    await page.screenshot({ path: test.info().outputPath("diff-desktop.png") });
    const geometry = await page.locator(".flow-columns").evaluate((element) => {
        const left = element
            .querySelector(".flow-scroll.old")!
            .getBoundingClientRect();
        const right = element
            .querySelector(".flow-scroll.new")!
            .getBoundingClientRect();
        const bridge = element
            .querySelector(".flow-bridge")!
            .getBoundingClientRect();
        const bounds = element.getBoundingClientRect();
        return {
            widthDifference: Math.abs(left.width - right.width),
            centerDifference: Math.abs(
                (bridge.left + bridge.right) / 2 -
                    (bounds.left + bounds.right) / 2,
            ),
        };
    });
    expect(geometry.widthDifference).toBeLessThanOrEqual(1);
    expect(geometry.centerDifference).toBeLessThanOrEqual(1);
    await expect(page.locator(".flow-scroll.old .flow-code-line")).toHaveCount(
        104,
    );
    await expect(page.locator(".flow-scroll.new .flow-code-line")).toHaveCount(
        85,
    );
    const left = page.locator(".flow-scroll.old");
    const right = page.locator(".flow-scroll.new");
    await left.evaluate((element) => {
        element.scrollTop = 850;
    });
    await expect
        .poll(() => right.evaluate((element) => element.scrollTop))
        .toBeGreaterThan(200);
    await page.getByRole("button", { name: "Unified", exact: true }).click();
    await expect(page.locator(".flow-unified-scroll")).toBeVisible();
    await page
        .getByRole("button", { name: "Side by side", exact: true })
        .click();
    await expect(page.locator(".flow-diff svg path").first()).toBeVisible();
    await page
        .getByRole("combobox", { name: "Changed file" })
        .selectOption("assets/receipt.png");
    await expect(
        page.getByText("Binary file — no text diff to display."),
    ).toBeVisible();
    expect(errors).toEqual([]);
});

test("diff viewer fits a narrow workbench without page overflow", async ({
    page,
}) => {
    await page.setViewportSize({ width: 760, height: 720 });
    await setup(page);
    const fits = await page.locator(".flow-diff").evaluate((element) => {
        const box = element.getBoundingClientRect();
        return box.left >= 0 && box.right <= window.innerWidth;
    });
    expect(fits).toBe(true);
    await page.screenshot({ path: test.info().outputPath("diff-narrow.png") });
});

test("native diff controls carry the pane and buffer identity", async ({
    page,
}) => {
    await page.route(
        "**/node_modules/.vite/deps/@tauri-apps_api_event.js*",
        (route) =>
            route.fulfill({
                contentType: "application/javascript",
                body: "export const listen = async () => () => {};",
            }),
    );
    await page.route(
        "**/node_modules/.vite/deps/@tauri-apps_api_core.js*",
        (route) =>
            route.fulfill({
                contentType: "application/javascript",
                body: `
                import { mockSnapshot } from '/src/mock.ts';
                export const isTauri = () => true;
                export class Channel {}
                window.diffCommands = [];
                let listener;
                export const invoke = async (command, args) => {
                    window.diffCommands.push({command, args});
                    if (command === 'gui_terminal_open') return 1;
                    if (command === 'gui_subscribe') {
                        listener = args.onEvent;
                        listener.onmessage(mockSnapshot);
                    }
                    if (command === 'gui_open_diff_source') listener.onmessage({
                        ...mockSnapshot,
                        revision: mockSnapshot.revision + 1,
                        panes: [{ ...mockSnapshot.panes[0], diffReview: undefined }],
                    });
                };`,
            }),
    );
    await setup(page);
    await expect(page.locator(".flow-diff")).toBeFocused();
    await page.keyboard.press("j");
    await expect
        .poll(() =>
            page
                .locator(".flow-scroll.old")
                .evaluate((element) => element.scrollTop),
        )
        .toBeGreaterThan(0);
    const scrollBeforeTerminal = await page
        .locator(".flow-scroll.old")
        .evaluate((element) => element.scrollTop);
    await page.keyboard.press("Control+Backquote");
    const shellInput = page.locator(".terminal-panel .xterm-helper-textarea");
    await expect(shellInput).toBeFocused();
    const toolbarFits = await page
        .locator(".flow-toolbar")
        .evaluate((toolbar) => {
            const bounds = toolbar.getBoundingClientRect();
            return Array.from(toolbar.querySelectorAll("button, select")).every(
                (control) => {
                    const rect = control.getBoundingClientRect();
                    return (
                        rect.left >= bounds.left && rect.right <= bounds.right
                    );
                },
            );
        });
    expect(toolbarFits).toBe(true);
    await page.keyboard.type("pwd");
    await expect
        .poll(() =>
            page.evaluate(() =>
                (window as any).diffCommands
                    .filter(
                        (item: any) => item.command === "gui_terminal_write",
                    )
                    .map((item: any) => item.args.data)
                    .join(""),
            ),
        )
        .toBe("pwd");
    expect(
        await page.evaluate(() =>
            (window as any).diffCommands.some(
                (item: any) => item.command === "gui_key",
            ),
        ),
    ).toBe(false);
    await page.screenshot({
        path: test.info().outputPath("diff-with-terminal.png"),
    });
    await page.keyboard.press("Control+Backquote");
    await expect(page.locator(".flow-diff")).toBeFocused();
    expect(
        await page
            .locator(".flow-scroll.old")
            .evaluate((element) => element.scrollTop),
    ).toBe(scrollBeforeTerminal);
    await page.keyboard.press(":");
    await expect(
        page.getByRole("textbox", { name: "Ovim editor input" }),
    ).toBeFocused();
    await expect
        .poll(() =>
            page.evaluate(() =>
                (window as any).diffCommands.some(
                    (item: any) =>
                        item.command === "gui_key" &&
                        item.args?.input?.key === ":",
                ),
            ),
        )
        .toBe(true);
    await page.getByRole("button", { name: "Unified", exact: true }).click();
    const action = await page.evaluate(
        () =>
            (window as any).diffCommands.find(
                (item: any) => item.command === "gui_diff_action",
            )?.args,
    );
    expect(action).toMatchObject({ pane: 0, action: "unified" });
    expect(action.bufferId).toEqual(expect.any(Number));
    await page
        .getByRole("button", {
            name: "After line 1204, open source",
            exact: true,
        })
        .click();
    await expect
        .poll(() =>
            page.evaluate(
                () =>
                    (window as any).diffCommands.find(
                        (item: any) => item.command === "gui_open_diff_source",
                    )?.args,
            ),
        )
        .toMatchObject({
            pane: 0,
            path: "src/payments/PaymentResource.java",
            line: 1204,
            side: "new",
        });
    await expect(page.locator(".flow-diff")).toHaveCount(0);
    await expect(
        page.getByRole("textbox", { name: "Ovim editor input" }),
    ).toBeFocused();
});

for (const view of ["Files", "Guided"]) {
    test(`exports a complete ${view} review as a PNG`, async ({
        page,
    }, testInfo) => {
        await setup(page, movedReview);
        await page.getByRole("button", { name: view, exact: true }).click();
        const pending = page.waitForEvent("download");
        await page
            .getByRole("button", { name: "Export image", exact: true })
            .click();
        const download = await pending;
        expect(download.suggestedFilename()).toMatch(/\.png$/);
        const destination = testInfo.outputPath("review.png");
        await download.saveAs(destination);
        const bytes = await readFile(destination);
        expect([...bytes.subarray(0, 8)]).toEqual([
            137, 80, 78, 71, 13, 10, 26, 10,
        ]);
        expect(bytes.readUInt32BE(16)).toBe(1600);
        expect(bytes.readUInt32BE(20)).toBeGreaterThan(420);
        await expect(page.locator(".flow-export-status")).toContainText(
            "Download started",
        );
    });
}

test("exports long reviews as one ZIP containing every numbered PNG page", async ({
    page,
}, testInfo) => {
    const longReview = structuredClone(review);
    const file = longReview.files[0];
    file.additions = 300;
    file.deletions = 0;
    file.hunks = [
        {
            oldStart: 0,
            oldCount: 0,
            newStart: 1,
            newCount: 300,
            header: "@@ -0,0 +1,300 @@",
            lines: Array.from({ length: 300 }, (_, i) => ({
                kind: "added",
                text: `export const value${i} = ${i};`,
                newLine: i + 1,
            })),
        },
    ];
    await setup(page, longReview);
    const pending = page.waitForEvent("download");
    await page
        .getByRole("button", { name: "Export image", exact: true })
        .click();
    const download = await pending;
    expect(download.suggestedFilename()).toMatch(/\.zip$/);
    const destination = testInfo.outputPath("review.zip");
    await download.saveAs(destination);
    const entries = Object.entries(unzipSync(await readFile(destination)));
    expect(entries.length).toBeGreaterThan(4);
    for (const [name, bytes] of entries) {
        expect(name).toMatch(/-\d+-of-\d+\.png$/);
        expect([...bytes.subarray(0, 8)]).toEqual([
            137, 80, 78, 71, 13, 10, 26, 10,
        ]);
    }
});
