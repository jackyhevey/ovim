import {
    replacementParts,
    hideEqualChanges,
    equalPairedLines,
    isEqualOnly,
    sectionsForFile,
    sectionsWithMoves,
    moveLocation,
    type FlowDiffFile,
    type FlowDiffLine,
    type FlowDiffMove,
    type FlowDiffReview,
    type Reconstruction,
} from "./FlowDiffModel";
import type { Side } from "./FlowDiffCode";

export type DiffExportView = "files" | "guided";

export type DiffExportOptions = {
    view: DiffExportView;
    reconstruction: Reconstruction;
    traceMoves?: boolean;
    hideEqual?: boolean;
};

export type DiffExportPage = {
    filename: string;
    svg: string;
    width: number;
    height: number;
};

export type RasterizedDiffExportPage = {
    filename: string;
    blob: Blob;
};

export class DiffExportTooLargeError extends Error {
    constructor(message: string) {
        super(message);
        this.name = "DiffExportTooLargeError";
    }
}

const PAGE_WIDTH = 1600;
const PAGE_HEIGHT = 2000;
const MARGIN = 54;
const COLUMN_GAP = 24;
const COLUMN_WIDTH = (PAGE_WIDTH - MARGIN * 2 - COLUMN_GAP) / 2;
const FONT_SIZE = 16;
const GLYPH_WIDTH = 10;
const CODE_CHARS = Math.floor((COLUMN_WIDTH - 82) / GLYPH_WIDTH);
const LINE_HEIGHT = 25;
const MONO = "'DejaVu Sans Mono', Menlo, Consolas, monospace";

type CodeCell = {
    number?: number;
    marker: string;
    kind: FlowDiffLine["kind"];
    text: string;
    fullText: string;
    counterpart?: string;
    offset: number;
    paired?: boolean;
};

type ExportRow =
    | {
          kind: "file";
          height: number;
          lines: string[];
          status: string;
          stats: string;
          continued?: boolean;
      }
    | {
          kind: "note";
          height: number;
          text: string;
          side?: Side;
          overlayTitle?: string;
      }
    | {
          kind: "gap";
          height: number;
          text: string;
          side?: Side;
          overlayTitle?: string;
      }
    | {
          kind: "code";
          height: number;
          left?: CodeCell;
          right?: CodeCell;
          overlaySide?: Side;
          overlayTitle?: string;
      }
    | {
          kind: "overlayHeading";
          height: number;
          text: string;
          side: Side;
          overlayTitle: string;
      };

type FileBlock = {
    file: FlowDiffFile;
    header: ExportRow & { kind: "file" };
    rows: ExportRow[];
};
type PositionedRow = { row: ExportRow; y: number };
type DraftPage = { rows: PositionedRow[]; bottom: number; file?: FlowDiffFile };

function xml(text: string): string {
    return printable(text)
        .replace(/&/g, "&amp;")
        .replace(/</g, "&lt;")
        .replace(/>/g, "&gt;")
        .replace(/"/g, "&quot;")
        .replace(/'/g, "&apos;");
}

function printable(text: string): string {
    return text.replace(/\t/g, "    ").replace(/[\u0000-\u001f\u007f]/g, "�");
}

function wrapCode(text: string): string[] {
    const characters = Array.from(printable(text));
    if (!characters.length) return [""];
    const chunks: string[] = [];
    for (let start = 0; start < characters.length; start += CODE_CHARS)
        chunks.push(characters.slice(start, start + CODE_CHARS).join(""));
    return chunks;
}

function wrapWords(text: string, limit: number): string[] {
    const words = printable(text).split(/\s+/).filter(Boolean);
    if (!words.length) return [""];
    const lines: string[] = [];
    let current = "";
    for (const word of words) {
        if (current && current.length + word.length + 1 > limit) {
            lines.push(current);
            current = "";
        }
        if (word.length > limit) {
            const chunks = Array.from(word);
            while (chunks.length > limit) {
                lines.push(chunks.splice(0, limit).join(""));
            }
            current = chunks.join("");
        } else {
            current = current ? `${current} ${word}` : word;
        }
    }
    if (current) lines.push(current);
    return lines;
}

function lineNumber(line: FlowDiffLine, side: Side): number | undefined {
    return side === "old" ? line.oldLine : line.newLine;
}

function codeCells(
    line: FlowDiffLine | undefined,
    side: Side,
    counterpart?: FlowDiffLine,
    paired = false,
): CodeCell[] {
    if (!line) return [];
    const fullText = printable(line.text);
    return wrapCode(line.text).map((text, index) => ({
        number: index === 0 ? lineNumber(line, side) : undefined,
        marker:
            index === 0
                ? line.kind === "added"
                    ? "+"
                    : line.kind === "removed"
                      ? "−"
                      : ""
                : "↳",
        kind: line.kind,
        text,
        fullText,
        counterpart: counterpart ? printable(counterpart.text) : undefined,
        offset: index * CODE_CHARS,
        paired,
    }));
}

function pairedRows(
    left: FlowDiffLine | undefined,
    right: FlowDiffLine | undefined,
): ExportRow[] {
    const leftCells = codeCells(left, "old", right);
    const rightCells = codeCells(right, "new", left);
    return Array.from(
        { length: Math.max(leftCells.length, rightCells.length) },
        (_, index) => ({
            kind: "code" as const,
            height: LINE_HEIGHT,
            left: leftCells[index],
            right: rightCells[index],
        }),
    );
}

function moveRows(
    move: FlowDiffMove,
    reconstruction: Reconstruction,
    anchorLines: FlowDiffLine[],
    hidden?: ReturnType<typeof equalPairedLines>,
): ExportRow[] {
    const endpoint = move[reconstruction];
    const side = reconstruction;
    const anchorSide = side === "old" ? "new" : "old";
    const anchor = move[anchorSide];
    const title = `Possible moved-code match · Before ${moveLocation(move.old)} → After ${moveLocation(move.new)}${move.label ? ` · ${move.label}` : ""}`;
    const rows: ExportRow[] = wrapWords(title, CODE_CHARS + 3).map((text) => ({
        kind: "overlayHeading",
        height: 30,
        text,
        side,
        overlayTitle: title,
    }));
    const firstWanted = Math.max(1, endpoint.startLine - 3);
    const lastWanted = endpoint.startLine + endpoint.lineCount + 2;
    const numbered = endpoint.contextWindows
        .flatMap((window) => window.lines)
        .filter((line) => {
            const number = lineNumber(line, side);
            return (
                number !== undefined &&
                number >= firstWanted &&
                number <= lastWanted
            );
        })
        .sort((a, b) => lineNumber(a, side)! - lineNumber(b, side)!);
    const sourceByNumber = new Map(
        numbered.map((line) => [lineNumber(line, side)!, line]),
    );
    const anchorByNumber = new Map(
        anchorLines.map((line) => [lineNumber(line, anchorSide)!, line]),
    );
    const contextLine = (line: FlowDiffLine) => {
        const cells = codeCells(line, side);
        for (const cell of cells)
            rows.push({
                kind: "code",
                height: LINE_HEIGHT,
                [side === "old" ? "left" : "right"]: cell,
                overlaySide: side,
                overlayTitle: title,
            });
    };
    const gap = (start: number, end: number) => {
        if (end < start) return;
        rows.push({
            kind: "gap",
            height: 26,
            text: `Lines ${start}–${end} outside saved context`,
            side,
            overlayTitle: title,
        });
    };
    let previous = firstWanted - 1;
    for (const line of numbered.filter(
        (item) => lineNumber(item, side)! < endpoint.startLine,
    )) {
        const number = lineNumber(line, side)!;
        if (number <= previous) continue;
        gap(previous + 1, number - 1);
        contextLine(line);
        previous = number;
    }
    for (
        let offset = 0;
        offset < Math.max(endpoint.lineCount, anchor.lineCount);
        offset++
    ) {
        const source =
            offset < endpoint.lineCount
                ? sourceByNumber.get(endpoint.startLine + offset)
                : undefined;
        if (offset < endpoint.lineCount && !source)
            throw new Error(
                `Moved source line ${endpoint.startLine + offset} is missing from saved context.`,
            );
        const canonical =
            offset < anchor.lineCount
                ? anchorByNumber.get(anchor.startLine + offset)
                : undefined;
        if (offset < anchor.lineCount && !canonical)
            throw new Error(
                `Canonical move line ${anchor.startLine + offset} is missing from this file.`,
            );
        const hiddenLine = (
            line: FlowDiffLine | undefined,
            lineSide: Reconstruction,
        ) =>
            line &&
            (lineSide === "old"
                ? hidden?.oldLines.has(line.oldLine!)
                : hidden?.newLines.has(line.newLine!));
        const sourceCells = hiddenLine(source, side)
            ? []
            : codeCells(source, side, canonical, Boolean(source));
        const anchorCells = hiddenLine(canonical, anchorSide)
            ? []
            : codeCells(canonical, anchorSide, source, Boolean(canonical));
        for (
            let index = 0;
            index < Math.max(sourceCells.length, anchorCells.length);
            index++
        )
            rows.push({
                kind: "code",
                height: LINE_HEIGHT,
                left: side === "old" ? sourceCells[index] : anchorCells[index],
                right: side === "old" ? anchorCells[index] : sourceCells[index],
                overlaySide: side,
                overlayTitle: title,
            });
    }
    previous = endpoint.startLine + endpoint.lineCount - 1;
    for (const line of numbered.filter(
        (item) => lineNumber(item, side)! > previous,
    )) {
        const number = lineNumber(line, side)!;
        if (number <= previous) continue;
        gap(previous + 1, number - 1);
        contextLine(line);
        previous = number;
    }
    if (!endpoint.contextComplete)
        rows.push({
            kind: "note",
            height: 28,
            text: "Showing nearby saved context",
            side,
            overlayTitle: title,
        });
    return rows;
}

function fileBlock(
    file: FlowDiffFile,
    moves: FlowDiffMove[],
    options: DiffExportOptions,
): FileBlock | undefined {
    const pathLines = wrapWords(file.path, 78);
    const header: ExportRow & { kind: "file" } = {
        kind: "file",
        height: 42 + (pathLines.length - 1) * 23,
        lines: pathLines,
        status: file.status,
        stats: `+${file.additions}  −${file.deletions}`,
    };
    const rows: ExportRow[] = [];
    if (file.label)
        for (const text of wrapWords(file.label, 130))
            rows.push({
                kind: "note",
                height: 28,
                text: `Description: ${text}`,
            });
    if (file.oldPath && file.oldPath !== file.path)
        for (const text of wrapWords(`Before path: ${file.oldPath}`, 130))
            rows.push({ kind: "note", height: 28, text });
    for (const metadata of file.metadata)
        for (const text of wrapWords(metadata, 130))
            rows.push({ kind: "note", height: 28, text });
    if (file.binary) {
        rows.push({
            kind: "note",
            height: 28,
            text: "Binary file; no text diff.",
        });
        return { file, header, rows };
    }
    const rawSections =
        options.view === "files" && options.traceMoves && moves.length
            ? sectionsWithMoves(file, moves, options.reconstruction)
            : sectionsForFile(file);
    const sections = options.hideEqual
        ? hideEqualChanges(file, rawSections, moves)
        : rawSections;
    if (options.hideEqual && isEqualOnly(file, sections)) return undefined;
    if (!sections.length)
        rows.push({
            kind: "note",
            height: 28,
            text: "No text changes in this file.",
        });
    const renderedMoves = new Set<string>();
    for (const section of sections) {
        if (section.kind === "gap") {
            rows.push({
                kind: "gap",
                height: 28,
                text: section.label || "Context omitted",
            });
            continue;
        }
        if (section.move) {
            const sourceSide = options.reconstruction;
            const sourceLines =
                sourceSide === "old" ? section.left : section.right;
            for (const line of sourceLines)
                rows.push(
                    ...pairedRows(
                        sourceSide === "old" ? line : undefined,
                        sourceSide === "new" ? line : undefined,
                    ),
                );
            if (renderedMoves.has(section.move.id)) continue;
            renderedMoves.add(section.move.id);
            const anchorSide = sourceSide === "old" ? "new" : "old";
            const anchor = section.move[anchorSide];
            const anchorLines = file.hunks
                .flatMap((hunk) => hunk.lines)
                .filter((line) => {
                    const number = lineNumber(line, anchorSide);
                    return (
                        line.kind ===
                            (anchorSide === "old" ? "removed" : "added") &&
                        number !== undefined &&
                        number >= anchor.startLine &&
                        number < anchor.startLine + anchor.lineCount
                    );
                });
            rows.push(
                ...moveRows(
                    section.move,
                    sourceSide,
                    anchorLines,
                    options.hideEqual
                        ? equalPairedLines(file, [section.move])
                        : undefined,
                ),
            );
            continue;
        }
        const count = Math.max(section.left.length, section.right.length);
        for (let index = 0; index < count; index++)
            rows.push(...pairedRows(section.left[index], section.right[index]));
    }
    return { file, header, rows };
}

function rowSvg(row: ExportRow, y: number): string {
    if (row.kind === "file") {
        const label = row.lines
            .map(
                (line, index) =>
                    `<text x="${MARGIN + 14}" y="${y + 27 + index * 23}" fill="#18253c" font-family="${MONO}" font-size="17" font-weight="700">${xml(line)}</text>`,
            )
            .join("");
        return `<rect x="${MARGIN}" y="${y}" width="${PAGE_WIDTH - 2 * MARGIN}" height="${row.height}" rx="5" fill="#e9eef5"/><text x="${PAGE_WIDTH - MARGIN - 14}" y="${y + 27}" text-anchor="end" fill="#52647e" font-family="${MONO}" font-size="14">${xml(`${row.status}   ${row.stats}${row.continued ? "   continued" : ""}`)}</text>${label}`;
    }
    if (row.kind === "overlayHeading") {
        const x =
            row.side === "old" ? MARGIN : MARGIN + COLUMN_WIDTH + COLUMN_GAP;
        return `<rect x="${x}" y="${y}" width="${COLUMN_WIDTH}" height="${row.height}" fill="#dfeaf6"/><rect x="${x}" y="${y}" width="4" height="${row.height}" fill="#547ba6"/><text x="${x + 14}" y="${y + 21}" fill="#294767" font-family="${MONO}" font-size="14" font-weight="700">${xml(row.text)}</text>`;
    }
    if (row.kind === "note" || row.kind === "gap") {
        const x =
            row.side === "new" ? MARGIN + COLUMN_WIDTH + COLUMN_GAP : MARGIN;
        const width = row.side ? COLUMN_WIDTH : PAGE_WIDTH - 2 * MARGIN;
        const fill = row.side
            ? "#f0f5fa"
            : row.kind === "gap"
              ? "#f2f5f8"
              : "#ffffff";
        return `<rect x="${x}" y="${y}" width="${width}" height="${row.height}" fill="${fill}"/><text x="${x + 13}" y="${y + 19}" fill="#607087" font-family="${MONO}" font-size="13">${xml(row.text)}</text>`;
    }
    const left = cellSvg(row.left, MARGIN, y, row.overlaySide === "old");
    const right = cellSvg(
        row.right,
        MARGIN + COLUMN_WIDTH + COLUMN_GAP,
        y,
        row.overlaySide === "new",
    );
    return left + right;
}

function cellSvg(
    cell: CodeCell | undefined,
    x: number,
    y: number,
    overlay: boolean,
): string {
    const fill = overlay
        ? "#f0f5fa"
        : cell?.kind === "added"
          ? "#eef8f0"
          : cell?.kind === "removed"
            ? "#fbefef"
            : "#ffffff";
    let svg = `<rect x="${x}" y="${y}" width="${COLUMN_WIDTH}" height="${LINE_HEIGHT}" fill="${fill}"/>`;
    if (!cell) return svg;
    if (cell.paired)
        svg += `<rect x="${x}" y="${y}" width="4" height="${LINE_HEIGHT}" fill="#547ba6"/>`;
    const codeX = x + 78;
    if (cell.counterpart !== undefined) {
        let offset = 0;
        for (const part of replacementParts(cell.fullText, cell.counterpart)) {
            const start = Math.max(cell.offset, offset);
            const end = Math.min(
                cell.offset + Array.from(cell.text).length,
                offset + Array.from(part.text).length,
            );
            if (part.changed && end > start)
                svg += `<rect x="${codeX + (start - cell.offset) * GLYPH_WIDTH}" y="${y + 4}" width="${(end - start) * GLYPH_WIDTH}" height="18" rx="2" fill="${cell.kind === "added" ? "#b8e3c1" : cell.kind === "removed" ? "#f2c4c4" : "#c7dcf2"}"/>`;
            offset += Array.from(part.text).length;
        }
    }
    const number = cell.number === undefined ? "" : String(cell.number);
    svg += `<text x="${x + 43}" y="${y + 18}" text-anchor="end" fill="#738198" font-family="${MONO}" font-size="13">${xml(number)}</text>`;
    svg += `<text x="${x + 58}" y="${y + 18}" fill="${cell.kind === "added" ? "#23834b" : cell.kind === "removed" ? "#b24a4a" : "#708097"}" font-family="${MONO}" font-size="14">${xml(cell.marker)}</text>`;
    if (cell.text)
        svg += `<text x="${codeX}" y="${y + 18}" fill="#1d2a3d" font-family="${MONO}" font-size="${FONT_SIZE}" textLength="${Array.from(cell.text).length * GLYPH_WIDTH}" lengthAdjust="spacingAndGlyphs" xml:space="preserve">${xml(cell.text)}</text>`;
    return svg;
}

function safeStem(title: string): string {
    const stem = title
        .normalize("NFKD")
        .replace(/[^a-zA-Z0-9]+/g, "-")
        .replace(/^-+|-+$/g, "")
        .toLowerCase()
        .slice(0, 64);
    return stem || "diff-review";
}

function changedLineCounts(files: FlowDiffFile[]): Map<string, number> {
    const counts = new Map<string, number>();
    for (const file of files) {
        for (const hunk of file.hunks) {
            for (const line of hunk.lines) {
                if (line.kind === "context") continue;
                const path =
                    line.kind === "removed"
                        ? file.oldPath || file.path
                        : file.path;
                const number =
                    line.kind === "removed" ? line.oldLine : line.newLine;
                const key = JSON.stringify([
                    path,
                    line.kind,
                    number,
                    line.text,
                ]);
                counts.set(key, (counts.get(key) || 0) + 1);
            }
        }
    }
    return counts;
}

function assertGuidedCoverage(
    canonical: FlowDiffFile[],
    guided: FlowDiffFile[],
): void {
    const expected = changedLineCounts(canonical);
    const actual = changedLineCounts(guided);
    if (
        expected.size !== actual.size ||
        [...expected].some(([key, count]) => actual.get(key) !== count)
    )
        throw new Error(
            "Guided sections do not cover every canonical changed line; image export was stopped.",
        );
    for (const file of canonical) {
        if (
            file.binary &&
            !guided.some((item) => item.path === file.path && item.binary)
        )
            throw new Error(`Guided sections omit binary file ${file.path}.`);
        for (const metadata of file.metadata) {
            if (
                !guided.some(
                    (item) =>
                        item.path === file.path &&
                        item.metadata.includes(metadata),
                )
            )
                throw new Error(
                    `Guided sections omit metadata for ${file.path}.`,
                );
        }
    }
}

/** Lay out every selected canonical line before creating any raster image. */
export function buildDiffExportPages(
    review: FlowDiffReview,
    options: DiffExportOptions,
): DiffExportPage[] {
    const files = options.view === "guided" ? review.guidedFiles : review.files;
    if (!files)
        throw new Error("Guided review is unavailable for this comparison.");
    if (options.view === "guided") assertGuidedCoverage(review.files, files);
    const moves = options.view === "files" ? review.moves || [] : [];
    const titleLines = wrapWords(review.title, 116);
    const bodyTop = 151 + titleLines.length * 24;
    if (bodyTop > PAGE_HEIGHT / 2)
        throw new DiffExportTooLargeError(
            "The review title is too long to fit a readable export page.",
        );
    const provenance = review.provenance;
    const source = provenance
        ? `Change accounting · Snapshot base: ${provenance.baseLabel} · ${provenance.comparisonBaseOid}${provenance.snapshotId ? ` · ${provenance.snapshotId}` : ""}`
        : "Change accounting from the selected review";
    const sourceLines = wrapWords(source, 115);
    const footerSpace = Math.max(74, 38 + sourceLines.length * 18);
    if (footerSpace > PAGE_HEIGHT / 3)
        throw new DiffExportTooLargeError(
            "Snapshot provenance is too long to fit a readable export page.",
        );
    const drafts: DraftPage[] = [{ rows: [], bottom: bodyTop }];
    let current = drafts[0];
    const append = (row: ExportRow, block: FileBlock) => {
        if (row.height > PAGE_HEIGHT - footerSpace - bodyTop)
            throw new DiffExportTooLargeError(
                "A review heading is too long to fit a readable export page.",
            );
        if (current.bottom + row.height > PAGE_HEIGHT - footerSpace) {
            current = { rows: [], bottom: bodyTop, file: block.file };
            drafts.push(current);
            if (row.kind !== "file") {
                const header = block.header;
                current.rows.push({
                    row: { ...header, continued: true },
                    y: current.bottom,
                });
                current.bottom += header.height;
            }
            if (
                "overlayTitle" in row &&
                row.overlayTitle &&
                row.kind !== "overlayHeading"
            ) {
                for (const text of wrapWords(
                    `${row.overlayTitle} (continued)`,
                    CODE_CHARS + 3,
                )) {
                    const heading: ExportRow = {
                        kind: "overlayHeading",
                        height: 30,
                        text,
                        side:
                            row.kind === "code"
                                ? row.overlaySide!
                                : row.side || "old",
                        overlayTitle: row.overlayTitle,
                    };
                    current.rows.push({ row: heading, y: current.bottom });
                    current.bottom += heading.height;
                }
            }
        }
        if (current.bottom + row.height > PAGE_HEIGHT - footerSpace)
            throw new DiffExportTooLargeError(
                "A review section is too tall to fit a readable export page.",
            );
        current.rows.push({ row, y: current.bottom });
        current.bottom += row.height;
        current.file = block.file;
    };
    for (const file of files) {
        const block = fileBlock(file, moves, options);
        if (!block) continue;
        append(block.header, block);
        for (const row of block.rows) append(row, block);
    }
    const additions = review.files.reduce(
        (sum, file) => sum + file.additions,
        0,
    );
    const deletions = review.files.reduce(
        (sum, file) => sum + file.deletions,
        0,
    );
    const moveCount = review.moves?.length ?? 0;
    const coverage = `${options.hideEqual ? "Equal same-file changes hidden · original patch: " : ""}${review.files.length} files · ${moveCount} possible moved-code matches · +${additions} −${deletions}`;
    const scope = options.view === "guided" ? "Guided sections" : "Files";
    const stem = safeStem(review.title);
    const pageCount = drafts.length;
    const pages = drafts.map((draft, index) => {
        const height = Math.min(
            PAGE_HEIGHT,
            Math.max(420, draft.bottom + footerSpace),
        );
        const header = titleLines
            .map(
                (line, lineIndex) =>
                    `<text x="${MARGIN}" y="${72 + lineIndex * 24}" fill="#18253c" font-family="${MONO}" font-size="20" font-weight="700">${xml(line)}</text>`,
            )
            .join("");
        const rows = draft.rows.length
            ? draft.rows.map(({ row, y }) => rowSvg(row, y)).join("")
            : rowSvg(
                  {
                      kind: "note",
                      height: 28,
                      text: options.hideEqual
                          ? "No unequal changes"
                          : "No changes",
                  },
                  bodyTop,
              );
        const footer = sourceLines
            .map(
                (line, lineIndex) =>
                    `<text x="${MARGIN}" y="${height - footerSpace + 32 + lineIndex * 18}" fill="#607087" font-family="${MONO}" font-size="12">${xml(line)}</text>`,
            )
            .join("");
        const viewLabel =
            options.view === "guided"
                ? "GUIDED SECTIONS"
                : options.traceMoves
                  ? options.reconstruction === "old"
                      ? "MOVED CODE · BEFORE CONTEXT"
                      : "MOVED CODE · AFTER CONTEXT"
                  : "FILE CHANGES";
        const svg = [
            `<svg xmlns="http://www.w3.org/2000/svg" width="${PAGE_WIDTH}" height="${height}" viewBox="0 0 ${PAGE_WIDTH} ${height}">`,
            `<rect width="${PAGE_WIDTH}" height="${height}" fill="#f8fafc"/>`,
            `<text x="${MARGIN}" y="38" fill="#547ba6" font-family="${MONO}" font-size="14" font-weight="700">DIFF REVIEW · ${xml(scope.toUpperCase())} · ${viewLabel}</text>`,
            header,
            `<text x="${MARGIN}" y="${bodyTop - 55}" fill="#607087" font-family="${MONO}" font-size="13">${xml(coverage)}</text>`,
            `<line x1="${MARGIN}" x2="${PAGE_WIDTH - MARGIN}" y1="${bodyTop - 44}" y2="${bodyTop - 44}" stroke="#d7dfe8"/>`,
            `<text x="${MARGIN + 8}" y="${bodyTop - 19}" fill="#52647e" font-family="${MONO}" font-size="13" font-weight="700">BEFORE</text>`,
            `<text x="${MARGIN + COLUMN_WIDTH + COLUMN_GAP + 8}" y="${bodyTop - 19}" fill="#52647e" font-family="${MONO}" font-size="13" font-weight="700">AFTER</text>`,
            rows,
            `<line x1="${MARGIN}" x2="${PAGE_WIDTH - MARGIN}" y1="${height - footerSpace + 12}" y2="${height - footerSpace + 12}" stroke="#d7dfe8"/>`,
            footer,
            `<text x="${PAGE_WIDTH - MARGIN}" y="${height - 34}" text-anchor="end" fill="#607087" font-family="${MONO}" font-size="12">Page ${index + 1} of ${pageCount}</text>`,
            "</svg>",
        ].join("");
        return {
            filename: `${stem}-${options.view}-${String(index + 1).padStart(2, "0")}-of-${String(pageCount).padStart(2, "0")}.png`,
            svg,
            width: PAGE_WIDTH,
            height,
        };
    });
    return pages;
}

/** Rasterize one page at a time so the canvas never holds the whole review. */
export async function rasterizeDiffExportPages(
    pages: DiffExportPage[],
): Promise<RasterizedDiffExportPage[]> {
    const output: RasterizedDiffExportPage[] = [];
    for (const page of pages) {
        const url = URL.createObjectURL(
            new Blob([page.svg], { type: "image/svg+xml;charset=utf-8" }),
        );
        try {
            const image = new Image();
            await new Promise<void>((resolve, reject) => {
                image.onload = () => resolve();
                image.onerror = () =>
                    reject(new Error(`Could not render ${page.filename}.`));
                image.src = url;
            });
            const canvas = document.createElement("canvas");
            canvas.width = page.width;
            canvas.height = page.height;
            try {
                const context = canvas.getContext("2d");
                if (!context)
                    throw new Error(
                        "Image export is unavailable in this browser.",
                    );
                context.drawImage(image, 0, 0);
                const blob = await new Promise<Blob>((resolve, reject) =>
                    canvas.toBlob(
                        (value) =>
                            value
                                ? resolve(value)
                                : reject(
                                      new Error(
                                          `Could not encode ${page.filename}.`,
                                      ),
                                  ),
                        "image/png",
                    ),
                );
                output.push({ filename: page.filename, blob });
            } finally {
                canvas.width = 0;
                canvas.height = 0;
            }
        } finally {
            URL.revokeObjectURL(url);
        }
    }
    return output;
}
