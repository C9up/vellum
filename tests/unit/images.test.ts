import { describe, expect, it } from "vitest";
import { PdfDocument, toBytes, Vellum, VellumError } from "../../src/index.js";
import { decodePng, encodePng, type Pixels } from "./png.js";

const vellum = new Vellum();

const RED: [number, number, number] = [255, 0, 0];
const BLUE: [number, number, number] = [0, 0, 255];
const WHITE: [number, number, number] = [255, 255, 255];

/** 40 x 20 pixels: red on the left half, blue on the right. */
const HALVES = encodePng(40, 20, (x) => (x < 20 ? RED : BLUE));

function page(): PdfDocument {
	return new PdfDocument({ size: [200, 200], margin: 0 });
}

async function pixels(doc: PdfDocument): Promise<Pixels> {
	const bytes = toBytes(doc);
	doc.end();
	return decodePng(await vellum.render(Buffer.from(await bytes), { page: 1 }));
}

function near(
	pixel: [number, number, number],
	expected: [number, number, number],
): boolean {
	return pixel.every(
		(value, index) => Math.abs(value - (expected[index] ?? 0)) < 40,
	);
}

describe("PdfDocument — images", () => {
	it("draws an image at its pixel size, then where it is told, at any size", async () => {
		const doc = page();
		doc.image(HALVES, 0, 0);
		doc.image(HALVES, 0, 100, { width: 200 });
		const image = await pixels(doc);
		expect(near(image.at(10, 10), RED)).toBe(true);
		expect(near(image.at(30, 10), BLUE)).toBe(true);
		expect(near(image.at(45, 10), WHITE), "40 points wide").toBe(true);
		// 200 wide keeps the 2:1 proportion: 100 high.
		expect(near(image.at(50, 150), RED)).toBe(true);
		expect(near(image.at(150, 195), BLUE)).toBe(true);
	});

	it("fits and covers a box, aligned in it, covering without overflowing", async () => {
		const doc = page();
		doc.image(HALVES, 0, 0, {
			fit: [100, 100],
			align: "center",
			valign: "center",
		});
		doc.image(HALVES, 100, 100, { cover: [50, 100] });
		doc.image(HALVES, 0, 150, { height: 20 });
		doc.image(HALVES, 150, 0, { scale: 0.5, opacity: 0.5 });
		const image = await pixels(doc);
		// Fit: 100 x 50, centred vertically in the 100 x 100 box.
		expect(near(image.at(20, 50), RED)).toBe(true);
		expect(near(image.at(20, 10), WHITE)).toBe(true);
		// Cover: 200 x 100 from (100, 100), cut to 50 x 100 — only red shows.
		expect(near(image.at(120, 150), RED)).toBe(true);
		expect(near(image.at(170, 150), WHITE), "cut off at the box").toBe(true);
		expect(near(image.at(5, 160), RED)).toBe(true);
		const faint = image.at(155, 5);
		expect(faint[1]).toBeGreaterThan(80);
	});

	it("moves the cursor down in the document flow, not with a y", () => {
		const doc = page();
		const top = doc.y;
		doc.image(HALVES);
		expect(doc.y - top).toBe(20);
		doc.image(HALVES, 0, 0);
		expect(doc.y - top).toBe(20);
	});

	it("opens an image once, from a path, a data URL or bytes", async () => {
		const doc = page();
		const opened = doc.openImage(HALVES);
		expect([opened.width, opened.height, opened.orientation]).toEqual([
			40, 20, 1,
		]);
		doc.image(opened, 0, 0);
		const url = `data:image/png;base64,${HALVES.toString("base64")}`;
		expect(doc.openImage(url)).toBe(doc.openImage(url));
		doc.image(url, 100, 0, { link: "https://c9up.dev" });
		const image = await pixels(doc);
		expect(near(image.at(110, 10), RED)).toBe(true);
		expect(() => page().image(opened)).toThrow(VellumError);
	});

	it("refuses what is not an image, or an image too large", () => {
		const doc = page();
		expect(() => doc.image(Buffer.from("not an image"))).toThrow(
			expect.objectContaining({ code: "E_VELLUM_INVALID_IMAGE" }),
		);
		const truncated = HALVES.subarray(0, HALVES.length - 20);
		expect(() => doc.image(truncated)).toThrow(VellumError);
	});
});
