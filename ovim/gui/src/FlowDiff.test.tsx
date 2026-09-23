// @vitest-environment jsdom
import { fireEvent, render, waitFor } from "@solidjs/testing-library";
import { describe, expect, it, vi } from "vitest";
import FlowDiff from "./FlowDiff";
import {
    mappedScrollTop,
    replacementParts,
    sectionsForFile,
    type FlowDiffReview,
} from "./FlowDiffModel";

const review: FlowDiffReview = {
    title: "main → working tree",
    layout: "split",
    managed: false,
    custom: false,
    files: [
        {
            id: "src/uneven.ts",
            path: "src/uneven.ts",
            status: "modified",
            additions: 1,
            deletions: 3,
            binary: false,
            metadata: [],
            hunks: [
                {
                    header: "@@ -4,4 +4,2 @@",
                    oldStart: 4,
                    oldCount: 4,
                    newStart: 4,
                    newCount: 2,
                    reviewLine: 20,
                    lines: [
                        {
                            kind: "context",
                            text: "same",
                            oldLine: 4,
                            newLine: 4,
                        },
                        {
                            kind: "removed",
                            text: "long version one",
                            oldLine: 5,
                        },
                        {
                            kind: "removed",
                            text: "long version two",
                            oldLine: 6,
                        },
                        {
                            kind: "removed",
                            text: "long version three",
                            oldLine: 7,
                        },
                        {
                            kind: "added",
                            text: "short version",
                            newLine: 5,
                            reviewLine: 23,
                        },
                    ],
                },
                {
                    header: "@@ -20 +18 @@",
                    oldStart: 20,
                    oldCount: 1,
                    newStart: 18,
                    newCount: 1,
                    reviewLine: 40,
                    lines: [
                        {
                            kind: "context",
                            text: "tail",
                            oldLine: 20,
                            newLine: 18,
                        },
                    ],
                },
            ],
        },
        {
            id: "asset.png",
            path: "asset.png",
            status: "modified",
            additions: 0,
            deletions: 0,
            binary: true,
            metadata: [],
            hunks: [],
        },
    ],
};

describe("flow diff model", () => {
    it("keeps unequal changes compact and pairs context and omitted ranges", () => {
        const sections = sectionsForFile(review.files[0]);
        expect(
            sections.map((section) => [
                section.kind,
                section.left.length,
                section.right.length,
            ]),
        ).toEqual([
            ["gap", 0, 0],
            ["context", 1, 1],
            ["change", 3, 1],
            ["gap", 0, 0],
            ["context", 1, 1],
        ]);
        expect(sections[3].label).toBe("12 unchanged lines");
    });

    it("maps scroll progress through sections of different heights", () => {
        expect(
            mappedScrollTop(
                75,
                [
                    { top: 0, height: 50 },
                    { top: 50, height: 100 },
                ],
                [
                    { top: 0, height: 50 },
                    { top: 50, height: 20 },
                ],
            ),
        ).toBe(55);
    });

    it("marks only replacement substrings without splitting Unicode characters", () => {
        expect(replacementParts("hello 👋 earth", "hello 🌍 earth")).toEqual([
            { text: "hello ", changed: false },
            { text: "👋", changed: true },
            { text: " earth", changed: false },
        ]);
        expect(replacementParts("an added line")).toEqual([
            { text: "an added line", changed: false },
        ]);
    });
});

describe("FlowDiff", () => {
    it("keeps cross-file sections distinct and opens each side at its own path", () => {
        const open = vi.fn();
        const moved: FlowDiffReview = {
            ...review,
            custom: true,
            files: [
                {
                    id: "pair_0",
                    label: "Extract parser",
                    path: "src/parser.ts",
                    oldPath: "src/main.ts",
                    status: "reassigned",
                    additions: 1,
                    deletions: 1,
                    binary: false,
                    metadata: ["new mode 100644"],
                    hunks: [
                        {
                            header: "Extract parser",
                            oldStart: 20,
                            oldCount: 1,
                            newStart: 3,
                            newCount: 1,
                            lines: [
                                {
                                    kind: "removed",
                                    text: "parse()",
                                    oldLine: 20,
                                },
                                { kind: "added", text: "parse()", newLine: 3 },
                            ],
                        },
                    ],
                },
                {
                    id: "residual_0",
                    path: "src/parser.ts",
                    status: "modified",
                    additions: 0,
                    deletions: 0,
                    binary: false,
                    metadata: [],
                    hunks: [],
                },
            ],
        };
        const result = render(() => (
            <FlowDiff review={moved} onOpenSource={open} />
        ));
        expect(
            result.getByRole("combobox", { name: "Diff section" }),
        ).toBeTruthy();
        expect(result.getByText("2 sections")).toBeTruthy();
        expect(result.getByText("new mode 100644")).toBeTruthy();
        fireEvent.click(
            result.getByRole("button", { name: "Before line 20, open source" }),
        );
        fireEvent.click(
            result.getByRole("button", { name: "After line 3, open source" }),
        );
        expect(open.mock.calls).toEqual([
            ["src/main.ts", 20, "old"],
            ["src/parser.ts", 3, "new"],
        ]);
        fireEvent.change(
            result.getByRole("combobox", { name: "Diff section" }),
            {
                target: { value: "residual_0" },
            },
        );
        expect(result.getByText("No text changes in this file.")).toBeTruthy();
    });

    it("shows independent compact streams and navigates source and hunks", () => {
        const navigate = vi.fn();
        const open = vi.fn();
        const result = render(() => (
            <FlowDiff
                review={review}
                onNavigateReviewLine={navigate}
                onOpenSource={open}
            />
        ));
        expect(
            result.container.querySelectorAll(
                ".flow-scroll.old .flow-code-line",
            ),
        ).toHaveLength(5);
        expect(
            result.container.querySelectorAll(
                ".flow-scroll.new .flow-code-line",
            ),
        ).toHaveLength(3);
        fireEvent.click(
            result.getByRole("button", { name: "Before line 5, open source" }),
        );
        expect(open).toHaveBeenCalledWith("src/uneven.ts", 5, "old");
        fireEvent.click(result.getByRole("button", { name: "Next change" }));
        expect(navigate).toHaveBeenCalledWith(40);
    });

    it("switches layout and handles binary files", () => {
        const layout = vi.fn();
        const result = render(() => (
            <FlowDiff review={review} onLayoutChange={layout} />
        ));
        fireEvent.click(result.getByRole("button", { name: "Unified" }));
        expect(layout).toHaveBeenCalledWith("unified");
        expect(
            result.container.querySelectorAll(".flow-unified-hunk"),
        ).toHaveLength(2);
        fireEvent.change(
            result.getByRole("combobox", { name: "Changed file" }),
            { target: { value: "asset.png" } },
        );
        expect(
            result.getByText("Binary file — no text diff to display."),
        ).toBeTruthy();
    });

    it("keeps review keys local and forwards command prefixes explicitly", () => {
        const action = vi.fn();
        const coreKey = vi.fn();
        const result = render(() => (
            <FlowDiff review={review} onAction={action} onCoreKey={coreKey} />
        ));
        const before = result.getByRole("region", { name: "Before changes" });
        fireEvent.keyDown(before, { key: " " });
        fireEvent.keyDown(before, { key: ":" });
        fireEvent.keyDown(before, { key: "q" });
        expect(coreKey.mock.calls).toEqual([[" "], [":"]]);
        expect(action).toHaveBeenCalledWith("q");
        fireEvent.click(result.getByRole("button", { name: "Refresh" }));
        expect(action).toHaveBeenCalledWith("r");
    });

    it("clicks the exact changed section and updates the counter on scroll", async () => {
        const navigate = vi.fn();
        const file = review.files[0];
        const document: FlowDiffReview = {
            ...review,
            files: [
                {
                    ...file,
                    hunks: [
                        {
                            ...file.hunks[0],
                            lines: [
                                {
                                    kind: "removed",
                                    text: "before a",
                                    oldLine: 4,
                                },
                                {
                                    kind: "added",
                                    text: "after a",
                                    newLine: 4,
                                    reviewLine: 23,
                                },
                                {
                                    kind: "context",
                                    text: "same",
                                    oldLine: 5,
                                    newLine: 5,
                                },
                                {
                                    kind: "removed",
                                    text: "before b",
                                    oldLine: 6,
                                },
                                {
                                    kind: "added",
                                    text: "after b",
                                    newLine: 6,
                                    reviewLine: 29,
                                },
                            ],
                        },
                    ],
                },
            ],
        };
        const result = render(() => (
            <FlowDiff review={document} onNavigateReviewLine={navigate} />
        ));
        const changes = result.container.querySelectorAll<HTMLElement>(
            ".flow-scroll.old .flow-section.change",
        );
        expect(changes).toHaveLength(2);
        Object.defineProperty(changes[1], "offsetTop", {
            configurable: true,
            value: 170,
        });
        const rightChange = result.container.querySelectorAll<HTMLElement>(
            ".flow-scroll.new .flow-section.change",
        )[1];
        Object.defineProperty(rightChange, "offsetTop", {
            configurable: true,
            value: 170,
        });
        Object.defineProperty(
            result.container.querySelector(".flow-bridge"),
            "clientHeight",
            {
                configurable: true,
                value: 300,
            },
        );
        const before = result.getByRole("region", { name: "Before changes" });
        await waitFor(() =>
            expect(
                result.container.querySelectorAll(".flow-ribbon"),
            ).toHaveLength(2),
        );
        fireEvent.click(result.container.querySelectorAll(".flow-ribbon")[1]);
        expect(before.scrollTop).toBe(162);
        expect(navigate).toHaveBeenCalledWith(29);
    });

    it("tracks the visible hunk while either pane scrolls", () => {
        const result = render(() => <FlowDiff review={review} />);
        const before = result.getByRole("region", { name: "Before changes" });
        const secondHunk = before.querySelector<HTMLElement>(
            "[data-section='h1-s4']",
        )!;
        Object.defineProperties(secondHunk, {
            offsetTop: { configurable: true, value: 100 },
            offsetHeight: { configurable: true, value: 22 },
        });
        before.scrollTop = 90;
        fireEvent.scroll(before);
        expect(result.getByText("2 / 2")).toBeTruthy();
    });
    it("opens the active change with Enter and gf without swallowing modified keys", () => {
        const openSource = vi.fn();
        const refresh = vi.fn();
        const result = render(() => (
            <FlowDiff
                review={review}
                onOpenSource={openSource}
                onAction={refresh}
            />
        ));
        const surface = result.getByRole("region", { name: "Diff review" });
        fireEvent.keyDown(surface, { key: "Enter" });
        expect(openSource).toHaveBeenLastCalledWith("src/uneven.ts", 5, "new");
        fireEvent.keyDown(surface, { key: "g" });
        fireEvent.keyDown(surface, { key: "f" });
        expect(openSource).toHaveBeenCalledTimes(2);
        fireEvent.keyDown(surface, { key: "r", metaKey: true });
        expect(refresh).not.toHaveBeenCalled();
    });
});
