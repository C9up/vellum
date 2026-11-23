import { Exception } from "@c9up/flare";
import { beforeAll, describe, expect, it } from "vitest";
import { A4, createBlank, errors, Vellum } from "../../src/index.js";

const A5 = { width: 419.53, height: 595.28 };

/**
 * A 1x1 red PNG, assembled from its chunks so no binary fixture is checked in.
 */
function redDot(): Buffer {
	return Buffer.from(
		"iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==",
		"base64",
	);
}

describe("Vellum — document operations", () => {
	let vellum: Vellum;

	beforeAll(() => {
		vellum = new Vellum();
	});

	it("merges documents end to end", async () => {
		const merged = await vellum.merge([
			createBlank([A4, A4]),
			createBlank([A4, A4, A4]),
		]);

		await expect(vellum.pageCount(merged)).resolves.toBe(5);
	});

	it("keeps each merged page its own size", async () => {
		const merged = await vellum.merge([createBlank([A4]), createBlank([A5])]);

		const sizes = await vellum.dimensions(merged);
		expect(sizes[0]?.width).toBeCloseTo(A4.width, 0);
		expect(sizes[1]?.width).toBeCloseTo(A5.width, 0);
	});

	it("refuses to merge nothing", async () => {
		try {
			await vellum.merge([]);
			expect.unreachable("merging nothing should be refused");
		} catch (error) {
			if (!(error instanceof Exception)) throw error;
			expect(error.code).toBe("E_VELLUM_EMPTY_MERGE");
		}
	});

	it("selects pages counting from 1", async () => {
		const source = createBlank([A4, A5, A4]);

		// Page 2 is the A5 one. If the public numbering ever slipped against
		// the engine's zero-based index, this would hand back an A4.
		const selected = await vellum.selectPages(source, [2]);

		const sizes = await vellum.dimensions(selected);
		expect(sizes).toHaveLength(1);
		expect(sizes[0]?.width).toBeCloseTo(A5.width, 0);
	});

	it("selects several pages", async () => {
		const selected = await vellum.selectPages(
			createBlank([A4, A4, A4, A4]),
			[1, 3],
		);

		await expect(vellum.pageCount(selected)).resolves.toBe(2);
	});

	it("refuses a page beyond the document", async () => {
		await expect(
			vellum.selectPages(createBlank([A4, A4]), [5]),
		).rejects.toThrow(/does not exist/);
	});

	it("refuses page 0, like every other page-addressing method", async () => {
		try {
			await vellum.selectPages(createBlank([A4]), [0]);
			expect.unreachable("page 0 should be refused");
		} catch (error) {
			if (!(error instanceof Exception)) throw error;
			expect(error.code).toBe("E_VELLUM_INVALID_PAGE");
		}
	});

	it("refuses a page number past 2^32 instead of wrapping it onto another page", async () => {
		// The engine takes 32-bit page indexes and JavaScript's conversion to
		// them wraps: 2^32 + 2 used to arrive as 1, page two, and every
		// page-addressing method acted on a page nobody named.
		const pdf = createBlank([A4, A5, A4]);
		const wrapped = 2 ** 32 + 2;
		const calls: Array<[string, () => Promise<unknown>]> = [
			["render", () => vellum.render(pdf, { page: wrapped })],
			["extractText", () => vellum.extractText(pdf, { page: wrapped })],
			["selectPages", () => vellum.selectPages(pdf, [wrapped])],
			["rotate", () => vellum.rotate(pdf, 90, { pages: [wrapped] })],
			["stamp", () => vellum.stamp(pdf, redDot(), { page: wrapped })],
			["stampText", () => vellum.stampText(pdf, "X", { page: wrapped })],
		];
		for (const [name, call] of calls) {
			try {
				await call();
				expect.unreachable(`${name} should refuse page ${wrapped}`);
			} catch (error) {
				if (!(error instanceof Exception)) throw error;
				expect(error.code, name).toBe("E_VELLUM_INVALID_PAGE");
				expect(error.message, name).toContain(String(wrapped));
			}
		}

		// The largest index the engine takes is still a page that does not
		// exist, reported in human numbering rather than wrapped to 0.
		await expect(vellum.selectPages(pdf, [2 ** 32])).rejects.toThrow(
			/page 4294967296 does not exist/,
		);
	});

	it("refuses an empty selection", async () => {
		await expect(vellum.selectPages(createBlank([A4]), [])).rejects.toThrow(
			errors.E_VELLUM_EMPTY_SELECTION,
		);
	});

	it("splits into one document per page", async () => {
		const parts = await vellum.split(createBlank([A4, A5, A4]));

		expect(parts).toHaveLength(3);
		for (const part of parts) {
			await expect(vellum.pageCount(part)).resolves.toBe(1);
		}
		// Each part keeps ITS page's size, not the first one's.
		const second = await vellum.dimensions(parts[1] as Buffer);
		expect(second[0]?.width).toBeCloseTo(A5.width, 0);
	});

	it("rotates and stays readable", async () => {
		const rotated = await vellum.rotate(createBlank([A4, A4]), 90);

		await expect(vellum.pageCount(rotated)).resolves.toBe(2);
	});

	it("rotates only the pages asked for", async () => {
		const rotated = await vellum.rotate(createBlank([A4, A4, A4]), 90, {
			pages: [2],
		});

		await expect(vellum.pageCount(rotated)).resolves.toBe(3);
	});

	it("refuses an angle that is not a quarter turn", async () => {
		for (const degrees of [45, 1, 90.5]) {
			try {
				await vellum.rotate(createBlank([A4]), degrees);
				expect.unreachable(`${degrees} should be refused`);
			} catch (error) {
				if (!(error instanceof Exception)) throw error;
				expect(error.code).toBe("E_VELLUM_INVALID_ROTATION");
			}
		}
	});

	it("stamps an image onto the document", async () => {
		// A 1x1 red PNG, written out byte by byte so the test carries no
		// binary fixture.
		const dot = redDot();

		const stamped = await vellum.stamp(createBlank([A4, A4]), dot, {
			page: 1,
			x: 50,
			y: 50,
			width: 100,
		});

		// Still a readable two-page document, and the page kept its size.
		await expect(vellum.pageCount(stamped)).resolves.toBe(2);
		const sizes = await vellum.dimensions(stamped);
		expect(sizes[0]?.width).toBeCloseTo(A4.width, 0);
	});

	it("refuses an image that is neither PNG nor JPEG", async () => {
		await expect(
			vellum.stamp(createBlank([A4]), Buffer.from("GIF89a"), {}),
		).rejects.toThrow(/PNG or JPEG/);
	});

	it("refuses a stamp page beyond the document", async () => {
		await expect(
			vellum.stamp(createBlank([A4]), redDot(), { page: 4 }),
		).rejects.toThrow(/does not exist/);
	});

	it("stamps every page when no page is named", async () => {
		// Absent page must mean "all", not "page 1" — the watermark case.
		const stamped = await vellum.stamp(createBlank([A4, A4]), redDot(), {
			width: 50,
		});

		await expect(vellum.pageCount(stamped)).resolves.toBe(2);
	});

	it("writes text that comes back out of the extractor", async () => {
		const marked = await vellum.stampText(createBlank([A4]), "PAYÉ", {
			page: 1,
			x: 400,
			y: 80,
			size: 24,
			color: "#c00",
		});

		// Written, encoded and read back — the whole round trip in one check.
		await expect(vellum.extractText(marked, { page: 1 })).resolves.toBe("PAYÉ");
	});

	it("refuses text a standard font cannot carry", async () => {
		// Refused rather than silently stripped: losing a character from a
		// contract is worse than failing.
		await expect(
			vellum.stampText(createBlank([A4]), "договор", {}),
		).rejects.toThrow(/WinAnsi/);
	});

	it("writes on every page when no page is named", async () => {
		const marked = await vellum.stampText(createBlank([A4, A4]), "BROUILLON", {
			x: 40,
			y: 100,
		});

		await expect(vellum.extractText(marked, { page: 2 })).resolves.toBe(
			"BROUILLON",
		);
	});

	it("reports no fields for a document without a form", async () => {
		// Most PDFs carry no AcroForm; absent is not an error.
		await expect(vellum.formFields(createBlank([A4]))).resolves.toEqual([]);
	});

	it("refuses bytes that are not a PDF when reading a form", async () => {
		await expect(vellum.formFields(Buffer.from("not a PDF"))).rejects.toThrow(
			errors.E_VELLUM_INVALID_PDF,
		);
	});

	it("refuses to fill a form that has no such field", async () => {
		// A document with no form at all: naming any field must fail rather
		// than quietly produce an unchanged file.
		await expect(
			vellum.fillForm(createBlank([A4]), { fullName: "x" }),
		).rejects.toThrow(/no field named/);
	});

	it("flattens a document that has no form at all", async () => {
		// Nothing to paint and nothing to remove: the document comes back
		// intact rather than failing for want of a form.
		const flat = await vellum.flattenForm(createBlank([A4, A4]));

		await expect(vellum.pageCount(flat)).resolves.toBe(2);
	});

	it("refuses bytes that are not a PDF when flattening", async () => {
		await expect(vellum.flattenForm(Buffer.from("not a PDF"))).rejects.toThrow(
			errors.E_VELLUM_FLATTEN_FAILED,
		);
	});

	it("refuses bytes that are not a PDF", async () => {
		const notAPdf = Buffer.from("not a PDF");

		await expect(vellum.split(notAPdf)).rejects.toThrow(
			errors.E_VELLUM_SPLIT_FAILED,
		);
		await expect(vellum.rotate(notAPdf, 90)).rejects.toThrow(
			errors.E_VELLUM_ROTATE_FAILED,
		);
		await expect(vellum.merge([notAPdf])).rejects.toThrow(
			errors.E_VELLUM_MERGE_FAILED,
		);
	});
});
