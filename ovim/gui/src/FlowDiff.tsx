import {
    For,
    Show,
    createEffect,
    createMemo,
    createSignal,
    onCleanup,
    onMount,
} from "solid-js";
import type { JSX } from "solid-js";
import {
    mappedScrollTop,
    replacementParts,
    sectionsForFile,
    type FlowDiffFile,
    type FlowDiffLine,
    type FlowDiffReview,
    type FlowSection,
} from "./FlowDiffModel";
import "./FlowDiff.css";

type Side = "old" | "new";

export type FlowDiffProps = {
    review: FlowDiffReview;
    syntax?: Record<string, string>;
    onNavigateReviewLine?: (line: number) => void;
    onOpenSource?: (path: string, line: number, side: Side) => void;
    onLayoutChange?: (layout: "split" | "unified") => void;
    onAction?: (key: "q" | "r") => void;
    onCoreKey?: (key: ":" | " ") => void;
};

function lineParts(line: FlowDiffLine, counterpart: FlowDiffLine | undefined) {
    return line.kind === "context"
        ? [{ text: line.text, changed: false }]
        : replacementParts(line.text, counterpart?.text);
}

function FileLine(props: {
    file: FlowDiffFile;
    line: FlowDiffLine;
    counterpart?: FlowDiffLine;
    side: Side;
    syntax?: Record<string, string>;
    onNavigateReviewLine?: (line: number) => void;
    onOpenSource?: (path: string, line: number, side: Side) => void;
}) {
    const number = () =>
        props.side === "old" ? props.line.oldLine : props.line.newLine;
    const canOpen = () => number() !== undefined && Boolean(props.onOpenSource);
    const open = () => {
        if (canOpen())
            props.onOpenSource?.(
                props.side === "old"
                    ? props.file.oldPath || props.file.path
                    : props.file.path,
                number()!,
                props.side,
            );
        else if (props.line.reviewLine !== undefined)
            props.onNavigateReviewLine?.(props.line.reviewLine);
    };
    const coloredText = () => {
        const parts = lineParts(props.line, props.counterpart);
        if (!props.syntax || !props.line.highlights?.length) {
            return parts.map((part) => (
                <span classList={{ "flow-word-change": part.changed }}>
                    {part.text}
                </span>
            ));
        }
        const boundaries = new Set([0, props.line.text.length]);
        for (const highlight of props.line.highlights) {
            boundaries.add(
                Math.max(0, Math.min(props.line.text.length, highlight.start)),
            );
            boundaries.add(
                Math.max(0, Math.min(props.line.text.length, highlight.end)),
            );
        }
        let offset = 0;
        for (const part of parts) {
            boundaries.add(offset);
            offset += part.text.length;
            boundaries.add(offset);
        }
        const edges = [...boundaries].sort((a, b) => a - b);
        return edges.slice(0, -1).map((start, index) => {
            const end = edges[index + 1];
            const highlight = props.line.highlights?.find(
                (item) => item.start <= start && item.end >= end,
            );
            const changed = parts.some((part, partIndex) => {
                const before = parts
                    .slice(0, partIndex)
                    .reduce((sum, item) => sum + item.text.length, 0);
                return (
                    part.changed &&
                    start >= before &&
                    end <= before + part.text.length
                );
            });
            return (
                <span
                    classList={{ "flow-word-change": changed }}
                    style={{
                        color: highlight
                            ? props.syntax?.[highlight.token]
                            : undefined,
                    }}
                >
                    {props.line.text.slice(start, end)}
                </span>
            );
        });
    };
    return (
        <div class={`flow-code-line ${props.line.kind}`}>
            <button
                type="button"
                class="flow-line-number"
                aria-label={`${props.side === "old" ? "Before" : "After"} line ${number() ?? "unavailable"}${canOpen() ? ", open source" : ""}`}
                disabled={
                    !canOpen() &&
                    (props.line.reviewLine === undefined ||
                        !props.onNavigateReviewLine)
                }
                onClick={open}
            >
                {number() ?? ""}
            </button>
            <span class="flow-line-mark" aria-hidden="true">
                {props.line.kind === "added"
                    ? "+"
                    : props.line.kind === "removed"
                      ? "−"
                      : ""}
            </span>
            <code class="flow-line-text">{coloredText()}</code>
        </div>
    );
}

export default function FlowDiff(props: FlowDiffProps) {
    const [selectedId, setSelectedId] = createSignal("");
    const [layout, setLayout] = createSignal<"split" | "unified">(
        props.review.layout,
    );
    const [ribbons, setRibbons] = createSignal<
        Array<{ id: string; path: string; kind: string }>
    >([]);
    const [activeHunk, setActiveHunk] = createSignal(0);
    let reviewIdentity = "";
    let leftScroller: HTMLDivElement | undefined;
    let rightScroller: HTMLDivElement | undefined;
    let bridge: HTMLDivElement | undefined;
    let unifiedScroller: HTMLDivElement | undefined;
    let lock = false;
    let pendingBracket = "";
    let pendingGo = false;
    let resizeObserver: ResizeObserver | undefined;
    const leftSections = new Map<string, HTMLElement>();
    const rightSections = new Map<string, HTMLElement>();
    const unifiedHunks = new Map<number, HTMLElement>();

    const file = createMemo(
        () =>
            props.review.files.find((item) => item.id === selectedId()) ||
            props.review.files[0],
    );
    const sections = createMemo(() => (file() ? sectionsForFile(file()!) : []));
    const hunks = createMemo(() => file()?.hunks ?? []);

    createEffect(() => setLayout(props.review.layout));
    createEffect(() => {
        const identity = `${props.review.title}\0${props.review.files.map((item) => item.id).join("\0")}`;
        if (reviewIdentity && identity !== reviewIdentity) setSelectedId("");
        reviewIdentity = identity;
    });
    createEffect(() => {
        file()?.path;
        setActiveHunk(0);
        queueMicrotask(() => {
            if (leftScroller) leftScroller.scrollTop = 0;
            if (rightScroller) rightScroller.scrollTop = 0;
            measure();
        });
    });

    function boxes(side: Side) {
        const scroller = side === "old" ? leftScroller : rightScroller;
        const elements = side === "old" ? leftSections : rightSections;
        return sections().map((section) => {
            const element = elements.get(section.id);
            return {
                top: element?.offsetTop ?? 0,
                height: element?.offsetHeight ?? 1,
                viewportTop:
                    (element?.offsetTop ?? 0) - (scroller?.scrollTop ?? 0),
            };
        });
    }

    function measure() {
        if (!bridge || layout() !== "split") return;
        const left = boxes("old");
        const right = boxes("new");
        const height = bridge.clientHeight;
        setRibbons(
            sections().flatMap((section, index) => {
                if (section.kind !== "change") return [];
                const a = left[index];
                const b = right[index];
                if (
                    !a ||
                    !b ||
                    (a.viewportTop > height && b.viewportTop > height) ||
                    (a.viewportTop + a.height < 0 &&
                        b.viewportTop + b.height < 0)
                )
                    return [];
                const topA = a.viewportTop;
                const topB = b.viewportTop;
                const bottomA = topA + Math.max(a.height, 3);
                const bottomB = topB + Math.max(b.height, 3);
                const width = bridge.clientWidth;
                const curve = width * 0.48;
                return [
                    {
                        id: section.id,
                        kind:
                            section.left.length && section.right.length
                                ? "replace"
                                : section.left.length
                                  ? "remove"
                                  : "add",
                        path: `M 0 ${topA} C ${curve} ${topA}, ${width - curve} ${topB}, ${width} ${topB} L ${width} ${bottomB} C ${width - curve} ${bottomB}, ${curve} ${bottomA}, 0 ${bottomA} Z`,
                    },
                ];
            }),
        );
    }

    function syncScroll(source: Side) {
        if (lock || !leftScroller || !rightScroller) return;
        lock = true;
        const from = source === "old" ? leftScroller : rightScroller;
        const to = source === "old" ? rightScroller : leftScroller;
        const visible = sections().find((section) => {
            const element = (
                source === "old" ? leftSections : rightSections
            ).get(section.id);
            return (
                element &&
                element.offsetTop + element.offsetHeight > from.scrollTop + 8
            );
        });
        if (visible) setActiveHunk(visible.hunkIndex);
        to.scrollTop = mappedScrollTop(
            from.scrollTop,
            boxes(source),
            boxes(source === "old" ? "new" : "old"),
        );
        measure();
        requestAnimationFrame(() => {
            lock = false;
        });
    }

    function goToSection(id: string, navigate = true) {
        const section = sections().find((item) => item.id === id);
        if (!section) return;
        setActiveHunk(section.hunkIndex);
        const left = leftSections.get(id);
        const right = rightSections.get(id);
        if (leftScroller && left)
            leftScroller.scrollTop = Math.max(0, left.offsetTop - 8);
        if (rightScroller && right)
            rightScroller.scrollTop = Math.max(0, right.offsetTop - 8);
        measure();
        const reviewLine = [...section.left, ...section.right].find(
            (line) => line.reviewLine !== undefined,
        )?.reviewLine;
        if (navigate && reviewLine !== undefined)
            props.onNavigateReviewLine?.(reviewLine);
    }

    function goToHunk(index: number) {
        const count = hunks().length;
        if (!count) return;
        const next = (index + count) % count;
        setActiveHunk(next);
        if (layout() === "unified") {
            const target = unifiedHunks.get(next);
            if (target && unifiedScroller)
                unifiedScroller.scrollTop = target.offsetTop;
        }
        const section = sections().find(
            (item) => item.hunkIndex === next && item.kind !== "gap",
        );
        if (section) goToSection(section.id, false);
        const reviewLine = hunks()[next]?.reviewLine;
        if (reviewLine !== undefined) props.onNavigateReviewLine?.(reviewLine);
    }

    function updateUnifiedActive() {
        if (!unifiedScroller) return;
        const threshold = unifiedScroller.scrollTop + 8;
        for (let index = hunks().length - 1; index >= 0; index--) {
            const element = unifiedHunks.get(index);
            if (element && element.offsetTop <= threshold) {
                setActiveHunk(index);
                return;
            }
        }
    }

    const sectionView = (section: FlowSection, side: Side) => {
        const lines = side === "old" ? section.left : section.right;
        const other = side === "old" ? section.right : section.left;
        return (
            <div
                class={`flow-section ${section.kind}`}
                data-section={section.id}
                ref={(element) =>
                    (side === "old" ? leftSections : rightSections).set(
                        section.id,
                        element,
                    )
                }
            >
                <Show when={section.kind === "gap"}>
                    <div class="flow-gap">
                        <span>···</span>
                        {section.label}
                    </div>
                </Show>
                <For each={lines}>
                    {(line, index) => (
                        <FileLine
                            file={file()!}
                            line={line}
                            counterpart={
                                section.kind === "change"
                                    ? other[index()]
                                    : undefined
                            }
                            side={side}
                            syntax={props.syntax}
                            onNavigateReviewLine={props.onNavigateReviewLine}
                            onOpenSource={props.onOpenSource}
                        />
                    )}
                </For>
                <Show when={section.kind === "change" && lines.length === 0}>
                    <div
                        class="flow-absence"
                        aria-label={`No ${side === "old" ? "removed" : "added"} lines in this section`}
                    >
                        <span>∅</span>
                    </div>
                </Show>
            </div>
        );
    };

    onMount(() => {
        if (typeof ResizeObserver !== "undefined")
            resizeObserver = new ResizeObserver(() => measure());
        queueMicrotask(measure);
    });
    onCleanup(() => resizeObserver?.disconnect());
    createEffect(() => {
        layout();
        queueMicrotask(() => {
            resizeObserver?.disconnect();
            if (bridge) resizeObserver?.observe(bridge);
            if (leftScroller) resizeObserver?.observe(leftScroller);
            if (rightScroller) resizeObserver?.observe(rightScroller);
            measure();
        });
    });

    const openCurrentChange = () => {
        const currentFile = file();
        const lines = hunks()[activeHunk()]?.lines;
        const line =
            lines?.find((item) => item.kind === "added") ??
            lines?.find((item) => item.kind === "removed") ??
            lines?.[0];
        if (!currentFile || !line) return;
        const side = line.kind === "removed" ? "old" : "new";
        const number = side === "old" ? line.oldLine : line.newLine;
        if (number !== undefined)
            props.onOpenSource?.(
                side === "old"
                    ? currentFile.oldPath || currentFile.path
                    : currentFile.path,
                number,
                side,
            );
    };

    const keydown: JSX.EventHandlerUnion<HTMLElement, KeyboardEvent> = (
        event,
    ) => {
        if (
            event.target instanceof HTMLSelectElement ||
            event.target instanceof HTMLButtonElement
        )
            return;
        if (event.metaKey || event.ctrlKey || event.altKey) return;
        if (event.key === "g") {
            pendingGo = true;
            return;
        }
        const openSource =
            event.key === "Enter" || (pendingGo && event.key === "f");
        pendingGo = false;
        if (openSource) {
            event.preventDefault();
            pendingBracket = "";
            openCurrentChange();
            return;
        }
        if (event.key === "[" || event.key === "]") {
            pendingBracket = event.key;
            return;
        }
        if (pendingBracket && (event.key === "c" || event.key === "f")) {
            event.preventDefault();
            if (event.key === "c")
                goToHunk(activeHunk() + (pendingBracket === "]" ? 1 : -1));
            else {
                const current = props.review.files.findIndex(
                    (item) => item.id === file()?.id,
                );
                const next =
                    (current +
                        (pendingBracket === "]" ? 1 : -1) +
                        props.review.files.length) %
                    props.review.files.length;
                setSelectedId(props.review.files[next]?.id ?? "");
            }
            pendingBracket = "";
            return;
        }
        pendingBracket = "";
        if (event.key === "F7" || event.key === "n") {
            event.preventDefault();
            goToHunk(activeHunk() + 1);
            return;
        }
        if (event.key === "N") {
            event.preventDefault();
            goToHunk(activeHunk() - 1);
            return;
        }
        if (event.key === "s") {
            event.preventDefault();
            const next = layout() === "split" ? "unified" : "split";
            setLayout(next);
            props.onLayoutChange?.(next);
            return;
        }
        if ((event.key === "q" || event.key === "r") && props.onAction) {
            event.preventDefault();
            props.onAction(event.key);
            return;
        }
        if (event.key === "j" || event.key === "k") {
            event.preventDefault();
            const step = event.key === "j" ? 22 : -22;
            if (layout() === "unified" && unifiedScroller)
                unifiedScroller.scrollTop += step;
            else if (leftScroller) {
                leftScroller.scrollTop += step;
                syncScroll("old");
            }
            return;
        }
        if ((event.key === ":" || event.key === " ") && props.onCoreKey) {
            event.preventDefault();
            props.onCoreKey(event.key);
        }
    };

    return (
        <section
            class="flow-diff"
            aria-label="Diff review"
            tabindex={0}
            data-gui-native-control
            onKeyDown={keydown}
        >
            <header class="flow-toolbar">
                <div class="flow-toolbar-title" title={props.review.title}>
                    <strong>Review</strong>
                    <span>{props.review.title}</span>
                </div>
                <div class="flow-toolbar-actions">
                    <label class="flow-file-picker">
                        <span>{props.review.custom ? "Section" : "File"}</span>
                        <select
                            aria-label={
                                props.review.custom
                                    ? "Diff section"
                                    : "Changed file"
                            }
                            value={file()?.id ?? ""}
                            onChange={(event) =>
                                setSelectedId(event.currentTarget.value)
                            }
                        >
                            <For each={props.review.files}>
                                {(item) => (
                                    <option value={item.id}>
                                        {item.label
                                            ? `${item.label} · ${item.path}`
                                            : item.path}
                                    </option>
                                )}
                            </For>
                        </select>
                    </label>
                    <span class="flow-file-count">
                        {props.review.files.length}{" "}
                        {props.review.custom
                            ? props.review.files.length === 1
                                ? "section"
                                : "sections"
                            : props.review.files.length === 1
                              ? "file"
                              : "files"}
                    </span>
                    <div class="flow-hunk-nav" aria-label="Change navigation">
                        <button
                            type="button"
                            aria-label="Previous change"
                            title="Previous change (Shift+N)"
                            disabled={!hunks().length}
                            onClick={() => goToHunk(activeHunk() - 1)}
                        >
                            ↑
                        </button>
                        <span>
                            {hunks().length ? activeHunk() + 1 : 0} /{" "}
                            {hunks().length}
                        </span>
                        <button
                            type="button"
                            aria-label="Next change"
                            title="Next change (F7 or N)"
                            disabled={!hunks().length}
                            onClick={() => goToHunk(activeHunk() + 1)}
                        >
                            ↓
                        </button>
                    </div>
                    <div
                        class="flow-layout-switch"
                        role="group"
                        aria-label="Diff layout"
                    >
                        <button
                            type="button"
                            aria-pressed={layout() === "split"}
                            onClick={() => {
                                setLayout("split");
                                props.onLayoutChange?.("split");
                                queueMicrotask(measure);
                            }}
                        >
                            Side by side
                        </button>
                        <button
                            type="button"
                            aria-pressed={layout() === "unified"}
                            onClick={() => {
                                setLayout("unified");
                                props.onLayoutChange?.("unified");
                            }}
                        >
                            Unified
                        </button>
                    </div>
                    <Show when={props.onAction}>
                        <div class="flow-action-buttons">
                            <button
                                type="button"
                                title={
                                    props.review.custom
                                        ? "Redraw saved review (R)"
                                        : "Refresh review (R)"
                                }
                                onClick={() => props.onAction?.("r")}
                            >
                                {props.review.custom ? "Redraw" : "Refresh"}
                            </button>
                            <button
                                type="button"
                                title="Close review (Q)"
                                onClick={() => props.onAction?.("q")}
                            >
                                Close
                            </button>
                        </div>
                    </Show>
                </div>
            </header>
            <Show
                when={file()}
                fallback={
                    <div class="flow-empty">
                        No changed files in this comparison.
                    </div>
                }
            >
                <div class="flow-file-heading">
                    <div>
                        <span class={`flow-status ${file()!.status}`}>
                            {file()!.status}
                        </span>
                        <strong>
                            {file()!.label
                                ? `${file()!.label} · ${file()!.path}`
                                : file()!.path}
                        </strong>
                        <Show
                            when={
                                file()!.oldPath &&
                                file()!.oldPath !== file()!.path
                            }
                        >
                            <span class="flow-renamed">
                                from {file()!.oldPath}
                            </span>
                        </Show>
                    </div>
                    <span class="flow-file-stats">
                        <b>+{file()!.additions}</b>
                        <i>−{file()!.deletions}</i>
                    </span>
                </div>
                <Show when={file()!.metadata?.length}>
                    <div class="flow-file-metadata" aria-label="File metadata">
                        <For each={file()!.metadata}>
                            {(line) => <code>{line}</code>}
                        </For>
                    </div>
                </Show>
                <Show
                    when={!file()!.binary}
                    fallback={
                        <div class="flow-empty">
                            Binary file — no text diff to display.
                        </div>
                    }
                >
                    <Show
                        when={hunks().length}
                        fallback={
                            <div class="flow-empty">
                                No text changes in this file.
                            </div>
                        }
                    >
                        <Show
                            when={layout() === "split"}
                            fallback={
                                <div
                                    class="flow-unified-scroll"
                                    ref={unifiedScroller}
                                    onScroll={updateUnifiedActive}
                                    role="region"
                                    aria-label="Unified changes"
                                    tabindex={0}
                                >
                                    <For each={file()!.hunks}>
                                        {(hunk, hunkIndex) => (
                                            <div
                                                class="flow-unified-hunk"
                                                ref={(element) =>
                                                    unifiedHunks.set(
                                                        hunkIndex(),
                                                        element,
                                                    )
                                                }
                                            >
                                                <div class="flow-unified-heading">
                                                    <span>
                                                        Change {hunkIndex() + 1}
                                                    </span>
                                                    <code>{hunk.header}</code>
                                                </div>
                                                <For each={hunk.lines}>
                                                    {(line) => (
                                                        <FileLine
                                                            file={file()!}
                                                            line={line}
                                                            side={
                                                                line.kind ===
                                                                "removed"
                                                                    ? "old"
                                                                    : "new"
                                                            }
                                                            syntax={
                                                                props.syntax
                                                            }
                                                            onNavigateReviewLine={
                                                                props.onNavigateReviewLine
                                                            }
                                                            onOpenSource={
                                                                props.onOpenSource
                                                            }
                                                        />
                                                    )}
                                                </For>
                                            </div>
                                        )}
                                    </For>
                                </div>
                            }
                        >
                            <div class="flow-columns">
                                <div class="flow-side-heading">
                                    <span>Before</span>
                                    <small>
                                        {file()!.oldPath || file()!.path}
                                    </small>
                                </div>
                                <div
                                    class="flow-bridge-heading"
                                    aria-hidden="true"
                                >
                                    ↝
                                </div>
                                <div class="flow-side-heading">
                                    <span>After</span>
                                    <small>{file()!.path}</small>
                                </div>
                                <div
                                    class="flow-scroll old"
                                    ref={leftScroller}
                                    onScroll={() => syncScroll("old")}
                                    role="region"
                                    aria-label="Before changes"
                                    tabindex={0}
                                >
                                    <For each={sections()}>
                                        {(section) =>
                                            sectionView(section, "old")
                                        }
                                    </For>
                                </div>
                                <div
                                    class="flow-bridge"
                                    ref={bridge}
                                    aria-hidden="true"
                                >
                                    <svg
                                        width="42"
                                        height="100%"
                                        preserveAspectRatio="none"
                                    >
                                        <For each={ribbons()}>
                                            {(ribbon) => (
                                                <path
                                                    class={`flow-ribbon ${ribbon.kind}`}
                                                    d={ribbon.path}
                                                    onClick={() =>
                                                        goToSection(ribbon.id)
                                                    }
                                                />
                                            )}
                                        </For>
                                    </svg>
                                </div>
                                <div
                                    class="flow-scroll new"
                                    ref={rightScroller}
                                    onScroll={() => syncScroll("new")}
                                    role="region"
                                    aria-label="After changes"
                                    tabindex={0}
                                >
                                    <For each={sections()}>
                                        {(section) =>
                                            sectionView(section, "new")
                                        }
                                    </For>
                                </div>
                            </div>
                        </Show>
                    </Show>
                </Show>
            </Show>
        </section>
    );
}
