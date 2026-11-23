import { Exception } from "@c9up/flare";
import { describe, expect, it } from "vitest";
import { errors } from "../../src/index.js";
import { compose } from "../../src/native.js";

function thrown(work: () => unknown): unknown {
	try {
		work();
	} catch (error) {
		return error;
	}
	return undefined;
}

describe("vellum > errors", () => {
	it("the entry exports every error as one `errors` namespace", () => {
		const error = new errors.E_VELLUM_INVALID_ROTATION([45]);
		expect(error).toBeInstanceOf(Exception);
		expect(error).toBeInstanceOf(errors.E_VELLUM_INVALID_ROTATION);
		expect(error.code).toBe("E_VELLUM_INVALID_ROTATION");
		expect(error.status).toBe(500);
		expect(error.message).toBe(
			"Rotation must be a whole multiple of 90, got 45.",
		);
	});

	it("bytes that are not a PDF answer 422", () => {
		const error = new errors.E_VELLUM_INVALID_PDF(["not a PDF"]);
		expect(error.status).toBe(422);
	});

	it("maps the engine's `CODE: reason` to the matching class, the engine error as cause", () => {
		const native = new Error("INVALID_LINK: no destination named intro");
		const error = thrown(() =>
			compose(() => {
				throw native;
			}),
		);
		expect(error).toBeInstanceOf(errors.E_VELLUM_INVALID_LINK);
		expect(error).toMatchObject({
			code: "E_VELLUM_INVALID_LINK",
			message: "no destination named intro",
			cause: native,
		});
	});

	it("keeps the whole message under E_VELLUM_WRITE_FAILED for a code it does not know", () => {
		const native = new Error("SOMETHING_NEW: reason");
		const error = thrown(() =>
			compose(() => {
				throw native;
			}),
		);
		expect(error).toBeInstanceOf(errors.E_VELLUM_WRITE_FAILED);
		expect(error).toMatchObject({ message: "SOMETHING_NEW: reason" });
	});
});
