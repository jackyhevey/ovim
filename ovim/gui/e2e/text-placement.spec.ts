import { expect, test } from "@playwright/test";

// Identical source with different cursor and syntax boundaries. Browser Range
// measurements check actual glyph origins against an unsegmented reference.
for (const mode of ["NORMAL", "INSERT"]) {
    test(`text stays on the same grid across ${mode} cursor and highlight boundaries`, async ({
        page,
    }) => {
        await page.route("**/src/mock.ts", async (route) => {
            const response = await route.fetch();
            await route.fulfill({
                response,
                body:
                    (await response.text()) +
                    `\n
const sample = "const result = value !== other;    return result;";
mockSnapshot.mode = ${JSON.stringify(mode)};
mockSnapshot.lines.splice(0, mockSnapshot.lines.length, ...[0, 6, 20, -1].map((cursor, row) => ({
    number: row + 1, continuation: false, displayStart: 0, current: cursor >= 0,
    segments: Array.from(sample).reduce((segments, text, index) => {
        const token = index < 5 ? "keyword" : index < 13 ? "variable" : undefined;
        const previous = segments.at(-1);
        if (previous && !previous.cursor && index !== cursor && previous.token === token) {
            previous.text += text; previous.cells++;
        } else segments.push({text, cells: 1, token, cursor: index === cursor, selected: false, searchMatch: false});
        return segments;
    }, [])
})));`,
            });
        });
        await page.goto("/");
        await expect(page.locator(".code-line")).toHaveCount(4);
        await page.evaluate(() => document.fonts.ready);
        const measure = () =>
            page.locator(".line-content").evaluateAll((lines) => {
                const reference = document.createElement("span");
                reference.textContent = lines[0].textContent;
                reference.style.cssText =
                    "position:absolute;white-space:pre;left:0";
                lines[0].append(reference);

                const positions = (element: Element) => {
                    const origin = element.getBoundingClientRect().left;
                    const walker = document.createTreeWalker(
                        element,
                        NodeFilter.SHOW_TEXT,
                    );
                    const xs: number[] = [];
                    while (walker.nextNode()) {
                        if (
                            reference.contains(walker.currentNode) &&
                            element !== reference
                        )
                            continue;
                        const node = walker.currentNode;
                        for (let i = 0; i < node.textContent!.length; i++) {
                            const range = document.createRange();
                            range.setStart(node, i);
                            range.setEnd(node, i + 1);
                            xs.push(
                                range.getBoundingClientRect().left - origin,
                            );
                        }
                    }
                    return xs;
                };
                const expected = positions(reference);
                const cell =
                    reference.getBoundingClientRect().width /
                    reference.textContent!.length;
                for (const line of lines) {
                    let column = 0;
                    for (const segment of line.querySelectorAll<HTMLElement>(
                        ".code-segment",
                    )) {
                        const x =
                            segment.getBoundingClientRect().left -
                            line.getBoundingClientRect().left;
                        if (Math.abs(x - column * cell) > 0.15)
                            throw new Error(
                                `Segment off grid: ${x} at column ${column}, cell ${cell}`,
                            );
                        column += segment.textContent!.length;
                    }
                }
                const errors = lines.map((line) =>
                    Math.max(
                        ...positions(line).map((x, i) =>
                            Math.abs(x - expected[i]),
                        ),
                    ),
                );

                reference.remove();
                return errors;
            });
        // WebKit rounds Range glyph bounds to pixels within each text run.
        for (const error of await measure()) expect(error).toBeLessThan(1);
        // Cursor paint and blinking cannot change any character's layout.
        await page.addStyleTag({
            content:
                ".code-segment.cursor { animation: none !important; box-shadow: none !important; }",
        });
        for (const error of await measure()) expect(error).toBeLessThan(1);
        // Font completion must reflow already-mounted segments, too.
        await page.addStyleTag({
            content: ".code-viewport { font-size: 17.5px; }",
        });
        await page.evaluate(() =>
            document.fonts.dispatchEvent(new Event("loadingdone")),
        );
        for (const error of await measure()) expect(error).toBeLessThan(1);
        await page.screenshot({
            path: test.info().outputPath("text-placement.png"),
        });
    });
}
