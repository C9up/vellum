import { beforeAll, describe, expect, it } from "vitest";
import { A4, createBlank, Vellum, VellumError } from "../../src/index.js";

const A5 = { width: 419.53, height: 595.28 };

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
			if (!(error instanceof VellumError)) throw error;
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
			if (!(error instanceof VellumError)) throw error;
			expect(error.code).toBe("E_VELLUM_INVALID_PAGE");
		}
	});

	it("refuses an empty selection", async () => {
		await expect(vellum.selectPages(createBlank([A4]), [])).rejects.toThrow(
			VellumError,
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
				if (!(error instanceof VellumError)) throw error;
				expect(error.code).toBe("E_VELLUM_INVALID_ROTATION");
			}
		}
	});

	it("refuses bytes that are not a PDF", async () => {
		const notAPdf = Buffer.from("not a PDF");

		await expect(vellum.split(notAPdf)).rejects.toThrow(VellumError);
		await expect(vellum.rotate(notAPdf, 90)).rejects.toThrow(VellumError);
		await expect(vellum.merge([notAPdf])).rejects.toThrow(VellumError);
	});
});
