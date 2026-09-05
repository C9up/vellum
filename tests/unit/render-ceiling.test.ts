/**
 * How large a page may rasterise to.
 *
 * Each side was bounded to 65535 and the buffer was not: two sides just inside
 * that limit are 16 GiB of RGBA between them. A page declares its own size, so
 * a document alone was enough to ask for it — with the default options, from
 * `render(pdf)` and nothing else.
 */
import { describe, expect, it } from "vitest";
import { A4, createBlank } from "../../src/index.js";
import { Vellum } from "../../src/Vellum.js";

describe("vellum > the size a page may rasterise to", () => {
	it("renders an ordinary page", async () => {
		const rendered = await new Vellum().render(createBlank([A4]));
		expect(rendered.length).toBeGreaterThan(0);
	});

	it("refuses a scale that asks for more pixels than allowed", async () => {
		const vellum = new Vellum();
		// A4 at scale 40 is 33k x 47k — inside the per-side limit, and 1.5
		// billion pixels between them.
		await expect(
			vellum.render(createBlank([A4]), { scale: 40 }),
		).rejects.toThrow(/million pixels, over the/);
	});

	it("names the ceiling and how to raise it", async () => {
		await expect(
			new Vellum().render(createBlank([A4]), { scale: 40 }),
		).rejects.toThrow(/raise maxPixels if you meant it/);
	});

	it("lets a caller who means it raise the ceiling", async () => {
		// Not to 1.5 billion — the point is that the knob is the caller's.
		const vellum = new Vellum({ maxPixels: 60_000_000 });
		const rendered = await vellum.render(createBlank([A4]), { scale: 8 });
		expect(rendered.length).toBeGreaterThan(0);
	});

	it("still refuses a page beyond the per-side limit", async () => {
		await expect(
			new Vellum().render(createBlank([A4]), { scale: 200 }),
		).rejects.toThrow(/exceeds the 65535px limit/);
	});
});
