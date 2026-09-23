import type { GuiDiffDocument, GuiDiffHunk, GuiDiffLine } from "./types";

export type FlowDiffLine = GuiDiffLine;
export type FlowDiffHunk = GuiDiffHunk;
export type FlowDiffReview = GuiDiffDocument;
export type FlowDiffFile = GuiDiffDocument["files"][number];
export type FlowDiffMove = NonNullable<GuiDiffDocument["moves"]>[number];
export type Reconstruction = "old" | "new";

export type FlowSection = {
    id: string;
    kind: "context" | "change" | "gap";
    hunkIndex: number;
    label?: string;
    left: FlowDiffLine[];
    right: FlowDiffLine[];
    move?: FlowDiffMove;
};

/** Keep move cards anchored to their exact canonical change lines. */
export function sectionsWithMoves(
    file: FlowDiffFile,
    moves: FlowDiffMove[],
    reconstruction: Reconstruction,
): FlowSection[] {
    const anchorSide = reconstruction === "old" ? "right" : "left";
    const endpointSide = reconstruction === "old" ? "new" : "old";
    const relevant = moves.filter(
        (move) =>
            move[endpointSide].path ===
            (endpointSide === "old" ? file.oldPath || file.path : file.path),
    );
    if (!relevant.length) return sectionsForFile(file);

    return sectionsForFile(file).flatMap((section) => {
        if (section.kind !== "change") return [section];
        const count = Math.max(section.left.length, section.right.length);
        const moveAt = (index: number) => {
            const number =
                section[anchorSide][index]?.[
                    reconstruction === "old" ? "newLine" : "oldLine"
                ];
            return relevant.find(
                (move) =>
                    number !== undefined &&
                    number >= move[endpointSide].startLine &&
                    number <
                        move[endpointSide].startLine +
                            move[endpointSide].lineCount,
            );
        };
        const parts: FlowSection[] = [];
        for (let start = 0; start < count;) {
            const move = moveAt(start);
            let end = start + 1;
            while (end < count && moveAt(end)?.id === move?.id) end++;
            parts.push({
                ...section,
                id: `${section.id}-p${parts.length}`,
                left: section.left.slice(start, end),
                right: section.right.slice(start, end),
                move,
            });
            start = end;
        }
        return parts;
    });
}

/** A section is the smallest unit whose two edges need to stay related. */
export function sectionsForFile(file: FlowDiffFile): FlowSection[] {
    const sections: FlowSection[] = [];
    let previousOldEnd = 1;
    let previousNewEnd = 1;

    file.hunks.forEach((hunk, hunkIndex) => {
        const skippedOld = Math.max(0, hunk.oldStart - previousOldEnd);
        const skippedNew = Math.max(0, hunk.newStart - previousNewEnd);
        if ((skippedOld || skippedNew) && file.status !== "reassigned") {
            sections.push({
                id: `gap-${hunkIndex}`,
                kind: "gap",
                hunkIndex,
                label: `${Math.max(skippedOld, skippedNew)} unchanged ${Math.max(skippedOld, skippedNew) === 1 ? "line" : "lines"}`,
                left: [],
                right: [],
            });
        }

        let run: FlowSection | undefined;
        for (const line of hunk.lines) {
            const kind = line.kind === "context" ? "context" : "change";
            if (!run || run.kind !== kind) {
                run = {
                    id: `h${hunkIndex}-s${sections.length}`,
                    kind,
                    hunkIndex,
                    left: [],
                    right: [],
                };
                sections.push(run);
            }
            if (line.kind !== "added") run.left.push(line);
            if (line.kind !== "removed") run.right.push(line);
        }
        previousOldEnd = hunk.oldStart + hunk.oldCount;
        previousNewEnd = hunk.newStart + hunk.newCount;
    });

    return sections;
}

export type InlinePart = { text: string; changed: boolean };

/** Highlight only the changed middle of a replacement pair. */
export function replacementParts(text: string, other?: string): InlinePart[] {
    if (other === undefined) return [{ text, changed: false }];
    if (text === other) return [{ text, changed: false }];
    const chars = Array.from(text);
    const otherChars = Array.from(other);
    let prefix = 0;
    while (
        prefix < chars.length &&
        prefix < otherChars.length &&
        chars[prefix] === otherChars[prefix]
    ) {
        prefix++;
    }
    let suffix = 0;
    while (
        suffix < chars.length - prefix &&
        suffix < otherChars.length - prefix &&
        chars[chars.length - suffix - 1] ===
            otherChars[otherChars.length - suffix - 1]
    ) {
        suffix++;
    }
    return [
        { text: chars.slice(0, prefix).join(""), changed: false },
        {
            text: chars.slice(prefix, chars.length - suffix).join(""),
            changed: true,
        },
        { text: chars.slice(chars.length - suffix).join(""), changed: false },
    ].filter((part) => part.text.length > 0);
}

/** Map a scroll position by section, retaining progress through unequal blocks. */
export function mappedScrollTop(
    sourceTop: number,
    source: Array<{ top: number; height: number }>,
    target: Array<{ top: number; height: number }>,
): number {
    if (!source.length || source.length !== target.length) return sourceTop;
    const index = source.findIndex(
        (section, i) =>
            sourceTop < section.top + section.height || i === source.length - 1,
    );
    const from = source[index];
    const to = target[index];
    const progress = Math.max(
        0,
        Math.min(1, (sourceTop - from.top) / Math.max(1, from.height)),
    );
    return to.top + progress * to.height;
}
