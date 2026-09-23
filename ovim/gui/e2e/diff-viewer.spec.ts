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
    files: [
        {
            path: "src/payments/PaymentResource.java",
            status: "modified",
            additions: 5,
            deletions: 24,
            binary: false,
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
            path: "assets/receipt.png",
            status: "modified",
            additions: 0,
            deletions: 0,
            binary: true,
            hunks: [],
        },
    ],
};

async function setup(page: Page) {
    await page.route("**/src/mock.ts", async (route) => {
        const response = await route.fetch();
        await route.fulfill({
            response,
            body: `${await response.text()}
                delete mockSnapshot.aiChat;
                delete mockSnapshot.fileTree;
                mockSnapshot.fileName = 'Diff · main → payment-api';
                mockSnapshot.filePath = undefined;
                mockSnapshot.readOnly = true;
                mockSnapshot.panes = [{...mockSnapshot.panes[0], diffReview: ${JSON.stringify(review)}}];`,
        });
    });
    await page.goto("/");
    await expect(page.locator(".flow-diff")).toBeVisible();
}

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
