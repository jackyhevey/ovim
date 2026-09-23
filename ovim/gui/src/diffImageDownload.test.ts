// @vitest-environment node
import { unzipSync } from "fflate";
import { describe, expect, it } from "vitest";
import { packageDiffImages } from "./diffImageDownload";

describe("diff image downloads", () => {
    it("keeps a single image unchanged and bundles every page without changing bytes", async () => {
        const images = [1, 2, 3].map((page) => ({
            filename: `review-${page}.png`,
            blob: new Blob([new Uint8Array([137, 80, 78, 71, page])], {
                type: "image/png",
            }),
        }));
        expect(await packageDiffImages(images.slice(0, 1))).toBe(images[0]);
        const archive = await packageDiffImages(images);
        const pages = unzipSync(
            new Uint8Array(await archive.blob.arrayBuffer()),
        );
        expect(Object.keys(pages)).toEqual(
            images.map((image) => image.filename),
        );
        for (const image of images)
            expect(pages[image.filename]).toEqual(
                new Uint8Array(await image.blob.arrayBuffer()),
            );
    });

    it("refuses duplicate or unsafe names instead of dropping a page", async () => {
        const image = { filename: "review.png", blob: new Blob(["png"]) };
        await expect(packageDiffImages([image, image])).rejects.toThrow(
            "unique",
        );
        await expect(
            packageDiffImages([{ ...image, filename: "../review.png" }]),
        ).rejects.toThrow("basenames");
    });
});
