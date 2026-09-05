/**
 * Rendering only part of a page.
 *
 * The reason this exists is a redaction: a scan is cropped so a signature or an
 * account number never leaves the building. So the one behaviour that matters
 * more than the crop itself is what happens when the band CANNOT be honoured —
 * a renderer that quietly hands back the whole page would leak exactly what the
 * band was there to remove, and nothing at the call site would say so.
 */
import { describe, expect, it } from "vitest";
import { A4, createBlank } from "../../src/index.js";
import { Vellum } from "../../src/Vellum.js";

/** PNG carries its pixel dimensions in the IHDR chunk, at a fixed offset. */
function pngSize(png: Buffer): { width: number; height: number } {
	return { width: png.readUInt32BE(16), height: png.readUInt32BE(20) };
}

const vellum = new Vellum();
const page = () => createBlank([A4]); // 595.28 x 841.89 pt

describe("vellum > rendering a band of a page", () => {
	it("hands back only the rectangle asked for", async () => {
		const full = pngSize(await vellum.render(page()));
		const band = pngSize(
			await vellum.render(page(), {
				band: { x: 0, y: 0, width: 595, height: 200 },
			}),
		);

		expect(band.width).toBe(595);
		expect(band.height).toBe(200);
		expect(band.height).toBeLessThan(full.height);
	});

	it("measures the band from the TOP-left, like stampText does", async () => {
		// Two coordinate systems in one package is how a redaction ends up
		// cropping the wrong end of the page.
		const top = await vellum.render(page(), {
			band: { x: 0, y: 0, width: 100, height: 100 },
		});
		const bottom = await vellum.render(page(), {
			band: { x: 0, y: 741, width: 100, height: 100 },
		});
		expect(pngSize(top)).toEqual({ width: 100, height: 100 });
		expect(pngSize(bottom)).toEqual({ width: 100, height: 100 });
	});

	it("scales the band with the page", async () => {
		const band = pngSize(
			await vellum.render(page(), {
				scale: 2,
				band: { x: 0, y: 0, width: 100, height: 50 },
			}),
		);
		expect(band).toEqual({ width: 200, height: 100 });
	});

	it("refuses a band that runs off the page rather than trimming it", async () => {
		// The dangerous case: a caller working from dimensions that are not this
		// document's. Answering with the overlap would answer a question nobody
		// asked, and look like it worked.
		await expect(
			vellum.render(page(), {
				band: { x: 0, y: 800, width: 595, height: 200 },
			}),
		).rejects.toThrow(/does not fit/);
	});

	it("refuses a band that starts outside the page", async () => {
		await expect(
			vellum.render(page(), {
				band: { x: -10, y: 0, width: 100, height: 100 },
			}),
		).rejects.toThrow(/outside the page/);
	});

	it("refuses a band with no area", async () => {
		await expect(
			vellum.render(page(), { band: { x: 0, y: 0, width: 0, height: 100 } }),
		).rejects.toThrow(/no area/);
	});

	it("refuses a band too small to be one pixel", async () => {
		await expect(
			vellum.render(page(), {
				scale: 0.01,
				band: { x: 0, y: 0, width: 1, height: 1 },
			}),
		).rejects.toThrow(/smaller than one pixel/);
	});

	it("crops a JPEG the same way", async () => {
		const jpeg = await vellum.render(page(), {
			format: "jpeg",
			band: { x: 10, y: 10, width: 200, height: 80 },
		});
		expect(jpeg.subarray(0, 2)).toEqual(Buffer.from([0xff, 0xd8]));
		expect(jpeg.length).toBeGreaterThan(0);
	});
});
