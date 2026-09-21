import { expect, test } from "@playwright/test";

// The native bridge fixture projects state after each command. Core tests own
// persistence and provider validation; this exercises the real rendered flow.
for (const viewport of [
    { width: 1154, height: 1054 },
    { width: 900, height: 700 },
]) {
    test(`provider permissions remain accessible at ${viewport.width}px`, async ({
        page,
    }, testInfo) => {
        await page.setViewportSize(viewport);
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
window.guiCommands = [];
let emit;
let revision = mockSnapshot.revision;
let chat = {
    ...mockSnapshot.aiChat,
    profile: 'claude_code', model: 'opus', externalAgent: true,
    externalQuestion: false, activity: 'idle', waiting: false,
    approval: undefined, setup: undefined, codeExplanation: undefined,
    input: '', inputCursor: 0, pendingImages: [], queuedInputs: [],
    reasoningEffort: 'high', reasoningEffortSelection: 'default',
    reasoningEffortDefault: 'high', reasoningEfforts: ['default', 'low', 'medium', 'high'],
    profiles: [
        {id:'claude_code', label:'Claude Agent', provider:'claude_code', model:'opus'},
        {id:'codex_sol', label:'Codex', provider:'codex', model:'gpt-5.6-sol'},
        ...Array.from({length: 12}, (_, index) => ({id:'custom-' + index, label:'Custom ' + index, provider:'ollama', model:'local-' + index})),
    ],
    permissionMode: 'auto',
    permissionModes: [
        {id:'auto', label:'Auto', description:'Let Claude classify approval requests.'},
        {id:'default', label:'Manual', description:'Ask before protected operations.'},
        {id:'acceptEdits', label:'Accept edits', description:'Approve ordinary file edits.'},
        {id:'plan', label:'Plan', description:'Explore and plan; ask before changing files.'},
        {id:'dontAsk', label:"Don't ask", description:'Deny calls that require approval.'},
        {id:'bypassPermissions', label:'Bypass permissions', description:'Skip ordinary approval prompts.'},
    ],
};
const publish = () => emit?.({...mockSnapshot, revision: ++revision, aiChat: chat});
export const invoke = async (command, args) => {
    window.guiCommands.push({command, args});
    if (command === 'gui_subscribe') { emit = args.onEvent.onmessage; publish(); }
    if (command === 'gui_select_permission_mode') { chat = {...chat, permissionMode: args.mode}; publish(); }
    if (command === 'gui_select_ai_profile' && args.profile === 'codex_sol') {
        chat = {...chat, profile: 'codex_sol', model: 'gpt-5.6-sol', externalAgent: false,
            permissionMode: undefined, permissionModes: []};
        publish();
    }
};`,
                }),
        );
        await page.goto("/");
        const trigger = page.getByRole("button", {
            name: /Claude Agent.*claude_code\/opus/,
        });
        await trigger.click();
        const dialog = page.getByRole("dialog", { name: "AI run settings" });
        const bounds = await dialog.boundingBox();
        expect(bounds).not.toBeNull();
        expect(bounds!.y + bounds!.height).toBeLessThanOrEqual(viewport.height);
        const permissions = page.getByRole("group", {
            name: "Permissions",
            exact: true,
        });
        await expect(permissions).toBeVisible();
        await expect(permissions.getByRole("button")).toHaveCount(6);
        await expect(
            permissions.getByRole("button", { name: /^Auto / }),
        ).toHaveAttribute("aria-pressed", "true");
        for (const name of [
            "Auto",
            "Manual",
            "Accept edits",
            "Plan",
            "Don't ask",
            "Bypass permissions",
        ]) {
            const option = permissions.getByRole("button", {
                name: new RegExp(`^${name} `),
            });
            await option.focus();
            await expect(option).toBeInViewport();
        }
        await permissions.getByRole("button", { name: /^Auto / }).focus();
        await page.screenshot({
            path: testInfo.outputPath("permissions-auto.png"),
            fullPage: true,
        });

        await permissions.getByRole("button", { name: /^Plan / }).focus();
        await page.keyboard.press("Enter");
        await expect(
            page.getByRole("dialog", { name: "AI run settings" }),
        ).toHaveCount(0);
        await expect(page.getByLabel("AI chat input")).toBeFocused();
        await expect
            .poll(() =>
                page.evaluate(
                    () =>
                        (window as any).guiCommands
                            .filter(
                                (entry: any) =>
                                    entry.command ===
                                    "gui_select_permission_mode",
                            )
                            .at(-1)?.args.mode,
                ),
            )
            .toBe("plan");
        await trigger.click();
        await expect(
            permissions.getByRole("button", { name: /^Plan / }),
        ).toHaveAttribute("aria-pressed", "true");
        await permissions
            .getByRole("button", { name: /^Bypass permissions / })
            .click();
        await expect
            .poll(() =>
                page.evaluate(
                    () =>
                        (window as any).guiCommands
                            .filter(
                                (entry: any) =>
                                    entry.command ===
                                    "gui_select_permission_mode",
                            )
                            .at(-1)?.args.mode,
                ),
            )
            .toBe("bypassPermissions");
        await trigger.click();
        await expect(
            permissions.getByRole("button", {
                name: /^Bypass permissions /,
            }),
        ).toHaveAttribute("aria-pressed", "true");
        await expect(
            permissions.getByRole("button", { name: /^Bypass permissions / }),
        ).toBeInViewport();
        await page.screenshot({
            path: testInfo.outputPath("permissions-bypass.png"),
            fullPage: true,
        });
        await page.getByRole("option", { name: /Codex/ }).click();
        await page
            .getByRole("button", { name: /Codex.*codex\/gpt-5.6-sol/ })
            .click();
        await expect(permissions).toHaveCount(0);
    });
}
