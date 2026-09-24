import { zipSync } from "fflate";

export type DiffImageFile = { filename: string; blob: Blob };

/** Keep a complete multi-page review together in one download. */
export async function packageDiffImages(
    images: DiffImageFile[],
): Promise<DiffImageFile> {
    if (!images.length)
        throw new Error("There are no review images to export.");
    const names = new Set<string>();
    for (const image of images) {
        if (
            !/^[\w-][\w.-]*\.png$/.test(image.filename) ||
            names.has(image.filename)
        )
            throw new Error(
                "Review image filenames must be unique PNG basenames.",
            );
        names.add(image.filename);
    }
    if (images.length === 1) return images[0];

    const entries = Object.fromEntries(
        await Promise.all(
            images.map(async (image) => [
                image.filename,
                new Uint8Array(await image.blob.arrayBuffer()),
            ]),
        ),
    );
    // PNG is already compressed; storing pages avoids recompressing each image.
    const archive = zipSync(entries, { level: 0 });
    return {
        filename: "ovim-diff-images.zip",
        blob: new Blob([new Uint8Array(archive)], { type: "application/zip" }),
    };
}

export function downloadDiffImages(file: DiffImageFile): void {
    const url = URL.createObjectURL(file.blob);
    const anchor = document.createElement("a");
    anchor.href = url;
    anchor.download = file.filename;
    document.body.append(anchor);
    anchor.click();
    anchor.remove();
    // WebKit may not consume the download URL until after the click dispatch.
    setTimeout(() => URL.revokeObjectURL(url), 30_000);
}
