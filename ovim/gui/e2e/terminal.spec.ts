import { expect, test } from "@playwright/test";

test("the activity bar and shortcut toggle the terminal", async ({
    page,
}, testInfo) => {
    await page.goto("/");
    const button = page.getByRole("button", { name: "Terminal", exact: true });

    await button.click();
    await expect(
        page.getByText("Terminal is available in the desktop app."),
    ).toBeVisible();
    await page.screenshot({
        path: testInfo.outputPath("terminal-preview.png"),
    });

    await page.keyboard.press("Control+Backquote");
    await expect(page.locator(".workbench")).toHaveClass(/terminal-collapsed/);
    await page.keyboard.press("Control+Backquote");
    await expect(
        page.getByText("Terminal is available in the desktop app."),
    ).toBeVisible();
    await expect(
        page.getByRole("button", { name: "Diff review", exact: true }),
    ).toHaveCount(0);
});
