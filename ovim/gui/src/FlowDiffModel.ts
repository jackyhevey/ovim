import { diffArrays } from "diff";
import type { GuiDiffDocument, GuiDiffHunk, GuiDiffLine } from "./types";

export type FlowDiffLine = GuiDiffLine;
export type FlowDiffHunk = GuiDiffHunk;
export type FlowDiffReview = GuiDiffDocument;
export type FlowDiffFile = GuiDiffDocument["files"][number];
export type FlowDiffMove = NonNullable<GuiDiffDocument["moves"]>[number];
export type Reconstruction = "old" | "new";

export function moveLocation(endpoint: FlowDiffMove["old"]): string {
    const end = endpoint.startLine + endpoint.lineCount - 1;
    return `${endpoint.path}:${endpoint.startLine}${end > endpoint.startLine ? `–${end}` : ""}`;
}

export type FlowSection = {
    id: string;
    kind: "context" | "change" | "gap";
    hunkIndex: number;
    label?: string;
    left: FlowDiffLine[];
    right: FlowDiffLine[];
    move?: FlowDiffMove;
    equal?: boolean;
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

function compareLines(before: FlowDiffLine[], after: FlowDiffLine[]) {
    const normalized = (line: FlowDiffLine) =>
        line.text.replace(/[ \t\r\f\v]/g, "");
    return diffArrays(before.map(normalized), after.map(normalized), {
        maxEditLength: 1000,
    });
}

/** The agent's same-file pairings may connect distant hunks. Use their source
 * coordinates to hide both endpoints in Files view as well as in Guided view.
 */
export function equalPairedLines(file: FlowDiffFile, moves: FlowDiffMove[]) {
    const oldLines = new Set<number>();
    const newLines = new Set<number>();
    const lines = file.hunks.flatMap((hunk) => hunk.lines);
    for (const move of moves) {
        if (move.old.path !== file.path || move.new.path !== file.path)
            continue;
        const before = lines.filter(
            (line) =>
                line.kind === "removed" &&
                line.oldLine !== undefined &&
                line.oldLine >= move.old.startLine &&
                line.oldLine < move.old.startLine + move.old.lineCount,
        );
        const after = lines.filter(
            (line) =>
                line.kind === "added" &&
                line.newLine !== undefined &&
                line.newLine >= move.new.startLine &&
                line.newLine < move.new.startLine + move.new.lineCount,
        );
        // Do not compare partial pairings when projecting a subset of the patch.
        if (
            before.length !== move.old.lineCount ||
            after.length !== move.new.lineCount
        )
            continue;
        let oldIndex = 0,
            newIndex = 0;
        for (const part of compareLines(before, after) ?? []) {
            if (!part.added && !part.removed) {
                for (let i = 0; i < part.count; i++) {
                    oldLines.add(before[oldIndex + i].oldLine!);
                    newLines.add(after[newIndex + i].newLine!);
                }
            }
            if (!part.added) oldIndex += part.count;
            if (!part.removed) newIndex += part.count;
        }
    }
    return { oldLines, newLines };
}

/** Collapse equal lines after ignoring horizontal whitespace, preserving source
 * coordinates and the original patch. Re-diff each replacement so cosmetic
 * edits do not obscure real changes inside the same block. Unpaired insertions and
 * deletions remain changes; cross-file moves retain their useful correspondence.
 */
export function hideEqualChanges(
    file: FlowDiffFile,
    sections: FlowSection[],
    moves: FlowDiffMove[] = [],
): FlowSection[] {
    // Cross-file correspondence remains useful even when the text is equal.
    if (file.oldPath && file.oldPath !== file.path) return sections;
    const paired = equalPairedLines(file, moves);
    return sections.flatMap((original) => {
        const left = original.left.filter(
            (line) => !paired.oldLines.has(line.oldLine!),
        );
        const right = original.right.filter(
            (line) => !paired.newLines.has(line.newLine!),
        );
        const hidden =
            original.left.length +
            original.right.length -
            left.length -
            right.length;
        const section = hidden ? { ...original, left, right } : original;
        if (hidden && !left.length && !right.length)
            return [
                {
                    id: original.id,
                    kind: "gap" as const,
                    equal: true,
                    hunkIndex: original.hunkIndex,
                    label: "Equal same-file pairing hidden",
                    left: [],
                    right: [],
                },
            ];
        if (
            section.kind !== "change" ||
            section.move ||
            !section.left.length ||
            !section.right.length
        )
            return [section];
        const diff = compareLines(section.left, section.right);
        // Very dissimilar blocks stay fully visible if the bounded comparison
        // cannot finish. Never discard a change without proving equality.
        if (!diff) return [section];
        let oldIndex = 0;
        let newIndex = 0;
        const result: FlowSection[] = [];
        let change: FlowSection | undefined;
        for (const part of diff) {
            const count = part.count;
            if (!part.added && !part.removed) {
                change = undefined;
                result.push({
                    id: `${section.id}-ws-${oldIndex}-${newIndex}`,
                    kind: "gap",
                    equal: true,
                    hunkIndex: section.hunkIndex,
                    label: `${count} ${count === 1 ? "line" : "lines"} unchanged ignoring whitespace`,
                    left: [],
                    right: [],
                });
                oldIndex += count;
                newIndex += count;
                continue;
            }
            if (!change) {
                change = {
                    id: `${section.id}-edit-${oldIndex}-${newIndex}`,
                    kind: "change",
                    hunkIndex: section.hunkIndex,
                    left: [],
                    right: [],
                };
                result.push(change);
            }
            if (part.removed) {
                change.left.push(
                    ...section.left.slice(oldIndex, oldIndex + count),
                );
                oldIndex += count;
            } else {
                change.right.push(
                    ...section.right.slice(newIndex, newIndex + count),
                );
                newIndex += count;
            }
        }
        return result;
    });
}

/** A wholly equal paired section should disappear, not leave an empty card.
 * Metadata and binary changes always remain available for review.
 */
export function isEqualOnly(
    file: FlowDiffFile,
    sections: FlowSection[],
): boolean {
    return (
        !file.binary &&
        !file.metadata.length &&
        sections.some((section) => section.equal) &&
        !sections.some((section) => section.kind === "change")
    );
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
