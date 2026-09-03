import { describe, expect, it } from "vitest";
import {
	A4,
	createBlank,
	inspect,
	isNativeAvailable,
	VellumError,
} from "../../src/index.js";

describe("vellum", () => {
	it("loads the native engine", () => {
		// Every other test here is meaningless if this is false: the package has
		// no JavaScript fallback, so a missing binary would turn the suite into
		// a set of assertions about error messages.
		expect(isNativeAvailable()).toBe(true);
	});

	it("reads back a document it authored", () => {
		const pdf = createBlank([A4, A4, { width: 210, height: 297 }]);

		const info = inspect(pdf);
		expect(info.pageCount).toBe(3);
		expect(info.encrypted).toBe(false);
		expect(info.version).toMatch(/^\d+\.\d+$/);
	});

	it("produces bytes a PDF reader would accept", () => {
		const pdf = createBlank([A4]);

		expect(pdf.subarray(0, 5).toString("latin1")).toBe("%PDF-");
		// A PDF ends with the end-of-file marker; a truncated write would not.
		expect(pdf.subarray(-6).toString("latin1")).toContain("%%EOF");
	});

	it("rejects bytes that are not a PDF", () => {
		expect(() => inspect(Buffer.from("this is not a PDF at all"))).toThrow(
			VellumError,
		);

		try {
			inspect(Buffer.from("this is not a PDF at all"));
			expect.unreachable("inspect should have thrown");
		} catch (error) {
			// `instanceof` rather than a cast: it narrows the type AND asserts it,
			// so a non-Vellum error fails here instead of being read as one.
			if (!(error instanceof VellumError)) throw error;
			expect(error.code).toBe("E_VELLUM_INVALID_PDF");
		}
	});

	it("refuses to author a document with no pages", () => {
		expect(() => createBlank([])).toThrow(VellumError);
	});

	it("refuses a page without area", () => {
		expect(() => createBlank([{ width: 0, height: 800 }])).toThrow(
			/must be positive/,
		);
	});
});
