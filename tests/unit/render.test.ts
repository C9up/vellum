import { beforeAll, describe, expect, it } from "vitest";
import { A4, createBlank, Vellum, VellumError } from "../../src/index.js";

/** Width and height out of a PNG's IHDR chunk, which starts at byte 16. */
function pngSize(bytes: Buffer): { width: number; height: number } {
	expect(bytes.subarray(0, 8)).toEqual(
		Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
	);
	return { width: bytes.readUInt32BE(16), height: bytes.readUInt32BE(20) };
}

function isJpeg(bytes: Buffer): boolean {
	return bytes[0] === 0xff && bytes[1] === 0xd8 && bytes[2] === 0xff;
}

describe("Vellum — rendering", () => {
	let pdf: Buffer;
	let vellum: Vellum;

	beforeAll(() => {
		pdf = createBlank([A4, A4, A4]);
		vellum = new Vellum();
	});

	it("renders a page to PNG at natural size", async () => {
		const image = await vellum.render(pdf);

		// A4 is 595.28 x 841.89pt; at 72 DPI that floors to 595x841.
		expect(pngSize(image)).toEqual({ width: 595, height: 841 });
	});

	it("counts pages from 1, not from 0", async () => {
		// The last page of a 3-page document is page 3 — the number a human
		// reads off the document, not an array index.
		await expect(vellum.render(pdf, { page: 3 })).resolves.toBeInstanceOf(
			Buffer,
		);

		await expect(vellum.render(pdf, { page: 4 })).rejects.toThrow(
			/does not exist/,
		);
	});

	it("refuses page 0 with a message about counting from 1", async () => {
		try {
			await vellum.render(pdf, { page: 0 });
			expect.unreachable("page 0 should be refused");
		} catch (error) {
			if (!(error instanceof VellumError)) throw error;
			expect(error.code).toBe("E_VELLUM_INVALID_PAGE");
			expect(error.message).toMatch(/start at 1/);
		}
	});

	it("honours a target width exactly and keeps the aspect ratio", async () => {
		const { width, height } = pngSize(
			await vellum.render(pdf, { width: 1200 }),
		);

		expect(width).toBe(1200);
		expect(height).toBeGreaterThanOrEqual(1696);
		expect(height).toBeLessThanOrEqual(1698);
	});

	it("renders every page", async () => {
		const images = await vellum.renderAll(pdf);

		expect(images).toHaveLength(3);
		for (const image of images) {
			expect(pngSize(image)).toEqual({ width: 595, height: 841 });
		}
	});

	it("renders JPEG when asked", async () => {
		const image = await vellum.render(pdf, { format: "jpeg", quality: 70 });

		expect(isJpeg(image)).toBe(true);
	});

	it("refuses a quality without a format instead of silently returning PNG", async () => {
		// A caller passing a quality wants a lossy image; answering with a
		// multi-megabyte PNG is a surprise nobody notices until previews crawl.
		await expect(vellum.render(pdf, { quality: 70 })).rejects.toThrow(
			/only applies to JPEG/,
		);
	});

	it("refuses a malformed background colour rather than defaulting to white", async () => {
		await expect(vellum.render(pdf, { background: "#gggggg" })).rejects.toThrow(
			/invalid colour/,
		);
	});

	it("refuses a scale that would exhaust memory", async () => {
		await expect(vellum.render(pdf, { scale: 1000 })).rejects.toThrow(
			/exceeds/,
		);
	});

	it("reports page dimensions in points", async () => {
		const pages = await vellum.dimensions(pdf);

		expect(pages).toHaveLength(3);
		expect(pages[0]?.width).toBeCloseTo(A4.width, 0);
		expect(pages[0]?.height).toBeCloseTo(A4.height, 0);
	});

	it("reports the page count", async () => {
		await expect(vellum.pageCount(pdf)).resolves.toBe(3);
	});
});

describe("Vellum — text extraction", () => {
	const blank = createBlank([A4, A4]);

	it("returns an empty string for a page with no text layer", async () => {
		// A scanned document without OCR is exactly this case: it has no text
		// to give, which is not an error.
		await expect(new Vellum().extractText(blank)).resolves.toBe("");
	});

	it("returns one entry per page", async () => {
		await expect(new Vellum().extractTextAll(blank)).resolves.toEqual(["", ""]);
	});

	it("shares the page numbering with render", async () => {
		const vellum = new Vellum();

		// Same guard, same code: proves #pageIndex is the single place the
		// public 1-based numbering meets the engine's 0-based index.
		for (const call of [
			() => vellum.extractText(blank, { page: 0 }),
			() => vellum.render(blank, { page: 0 }),
		]) {
			try {
				await call();
				expect.unreachable("page 0 should be refused");
			} catch (error) {
				if (!(error instanceof VellumError)) throw error;
				expect(error.code).toBe("E_VELLUM_INVALID_PAGE");
			}
		}
	});

	it("refuses a page beyond the document", async () => {
		await expect(new Vellum().extractText(blank, { page: 3 })).rejects.toThrow(
			/does not exist/,
		);
	});

	it("refuses bytes that are not a PDF", async () => {
		await expect(
			new Vellum().extractText(Buffer.from("not a PDF")),
		).rejects.toThrow(VellumError);
	});
});

describe("Vellum — metadata", () => {
	it("reports nothing for a document that carries no /Info", async () => {
		// A PDF is valid with no /Info dictionary, and krilla writes none.
		// Absent must not be an error, or every generated document would fail.
		const info = await new Vellum().metadata(createBlank([A4]));

		// napi maps `Option::None` to an ABSENT key, not to null — the object
		// comes back as `{}`, so every field reads undefined.
		expect(info.title).toBeUndefined();
		expect(info.author).toBeUndefined();
		expect(info.createdAt).toBeUndefined();
	});

	it("refuses bytes that are not a PDF", async () => {
		await expect(
			new Vellum().metadata(Buffer.from("not a PDF")),
		).rejects.toThrow(VellumError);
	});
});

describe("Vellum — configuration", () => {
	const pdf = createBlank([A4]);

	it("applies the configured defaults", async () => {
		const vellum = new Vellum({ format: "jpeg", quality: 60 });

		expect(isJpeg(await vellum.render(pdf))).toBe(true);
	});

	it("lets a call override the configuration", async () => {
		const vellum = new Vellum({ format: "jpeg", quality: 60, width: 400 });

		const image = await vellum.render(pdf, { format: "png", width: 900 });
		expect(pngSize(image).width).toBe(900);
	});

	it("exposes the configuration it resolved", () => {
		const config = { format: "jpeg", quality: 60 } as const;

		expect(new Vellum(config).config).toEqual(config);
	});
});
