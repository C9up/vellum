/**
 * Documents built to take the process down.
 *
 * A panic in the engine is caught and becomes a rejected promise. A stack
 * overflow is not a panic: it ends the process, every request in it included.
 * Each document below once did; it must now come back as an answer.
 */
import { describe, expect, it } from "vitest";
import { Vellum } from "../../src/Vellum.js";
import { buildPdf, HELVETICA, onePage, stream } from "./rawPdf.js";

const DEEP = 200_000;

describe("vellum > documents nested to overflow the stack", () => {
	const vellum = new Vellum();

	it("refuses a page whose resources nest arrays 200 000 deep", async () => {
		const pdf = onePage(
			stream("", "q Q"),
			`/Foo ${"[".repeat(DEEP)}${"]".repeat(DEEP)}`,
		);
		await expect(vellum.render(pdf)).rejects.toThrow(/page 1 does not exist/);
		await expect(vellum.extractText(pdf)).rejects.toThrow(
			/page 1 does not exist/,
		);
		await expect(vellum.renderAll(pdf)).resolves.toEqual([]);
		expect(await vellum.dimensions(pdf)).toEqual([]);
	});

	it("refuses a page whose resources nest dictionaries 200 000 deep", async () => {
		const pdf = onePage(
			stream("", "q Q"),
			`/Foo ${"<< /A ".repeat(DEEP)}1${" >>".repeat(DEEP)}`,
		);
		await expect(vellum.render(pdf)).rejects.toThrow(/page 1 does not exist/);
		expect(await vellum.dimensions(pdf)).toEqual([]);
	});

	it("renders a page whose content stream nests arrays 200 000 deep, without them", async () => {
		const pdf = onePage(
			stream("", `${"[".repeat(DEEP)}${"]".repeat(DEEP)} pop q Q`),
			HELVETICA,
		);
		const png = await vellum.render(pdf);
		expect(png.subarray(1, 4).toString("latin1")).toBe("PNG");
		await expect(vellum.extractText(pdf)).resolves.toBe("");
	});

	it("renders a page whose colour function nests braces 200 000 deep, without it", async () => {
		const body = `{${"{".repeat(DEEP)}${"}".repeat(DEEP)} pop 0 }`;
		const pdf = onePage(
			stream("", "/CS0 cs 0.5 sc 0 0 100 100 re f"),
			"/ColorSpace << /CS0 [/Separation /Ink /DeviceGray 5 0 R] >>",
			[stream("/FunctionType 4 /Domain [0 1] /Range [0 1]", body)],
		);
		const png = await vellum.render(pdf);
		expect(png.subarray(1, 4).toString("latin1")).toBe("PNG");
	});

	it("is still alive to render an ordinary page afterwards", async () => {
		const pdf = onePage(
			stream("", "BT /F1 12 Tf 72 720 Td (still here) Tj ET"),
			HELVETICA,
		);
		await expect(vellum.extractText(pdf)).resolves.toBe("still here");
	});
});

/** A form that is a chain of `depth` fields, each the only child of the last. */
function fieldChain(depth: number): Buffer {
	const objects: string[] = [
		"<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R] >> >>",
		"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
		"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] >>",
	];
	for (let level = 0; level < depth; level++) {
		objects.push(`<< /T (a) /Kids [${5 + level} 0 R] >>`);
	}
	objects.push("<< /T (leaf) /FT /Tx /V (x) >>");
	return buildPdf(objects);
}

describe("vellum > a form tree deep enough to overflow the stack", () => {
	const vellum = new Vellum();
	const deep = fieldChain(30_000);

	it("refuses to read it", async () => {
		await expect(vellum.formFields(deep)).rejects.toThrow(
			/deeper than 64 levels/,
		);
	});

	it("refuses to fill, flatten or verify it", async () => {
		await expect(vellum.fillForm(deep, { "a.leaf": "y" })).rejects.toThrow(
			/deeper than 64/,
		);
		await expect(vellum.flattenForm(deep)).rejects.toThrow(/deeper than 64/);
		await expect(vellum.verifySignatures(deep)).rejects.toThrow(
			/deeper than 64/,
		);
	});

	it("still reads a form of ordinary depth", async () => {
		const fields = await vellum.formFields(fieldChain(3));
		expect(fields.map((field) => field.name)).toEqual(["a.a.a.leaf"]);
	});
});

const MIB = 1024 * 1024;

/** A page whose content is `copies` references to one Flate stream. */
function flateBomb(inflated: number, copies: number): Buffer {
	const contents = Array.from({ length: copies }, () => "4 0 R").join(" ");
	return buildPdf([
		"<< /Type /Catalog /Pages 2 0 R >>",
		"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
		`<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Contents [${contents}] /Resources << ${HELVETICA} >> >>`,
		stream("", Buffer.alloc(inflated, " "), true),
	]);
}

describe("vellum > a stream that inflates past the decoding budget", () => {
	const vellum = new Vellum({ maxDecodedBytes: MIB });

	it("is refused when rendering and reading text", async () => {
		const pdf = flateBomb(16 * MIB, 1);
		await expect(vellum.render(pdf)).rejects.toThrow(/more than 1 MiB/);
		await expect(vellum.renderAll(pdf)).rejects.toThrow(
			/raise maxDecodedBytes/,
		);
		await expect(vellum.extractText(pdf)).rejects.toThrow(/more than 1 MiB/);
		await expect(vellum.extractTextAll(pdf)).rejects.toThrow(/more than 1 MiB/);
	});

	it("is charged for every reference to it", async () => {
		await expect(vellum.render(flateBomb(256 * 1024, 8))).rejects.toThrow(
			/more than 1 MiB/,
		);
		await expect(
			vellum.render(flateBomb(256 * 1024, 1)),
		).resolves.toBeInstanceOf(Buffer);
	});

	it("can be raised for one call", async () => {
		const pdf = flateBomb(2 * MIB, 1);
		await expect(vellum.render(pdf)).rejects.toThrow(/more than 1 MiB/);
		await expect(
			vellum.render(pdf, { maxDecodedBytes: 4 * MIB }),
		).resolves.toBeInstanceOf(Buffer);
	});
});

/** Form XObjects each drawing the next ten times, `levels` deep. */
function fanOut(levels: number): Buffer {
	const forms: Buffer[] = [];
	for (let level = 0; level < levels; level++) {
		const last = level + 1 === levels;
		const body = last
			? "BT /F1 1 Tf (a) Tj ET 0 0 1 1 re f"
			: Array(10).fill("/X Do").join(" ");
		const resources = last ? HELVETICA : `/XObject << /X ${6 + level} 0 R >>`;
		forms.push(
			stream(
				`/Type /XObject /Subtype /Form /BBox [0 0 10 10] /Resources << ${resources} >>`,
				body,
			),
		);
	}
	return buildPdf([
		"<< /Type /Catalog /Pages 2 0 R >>",
		"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
		"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Contents 4 0 R /Resources << /XObject << /X 5 0 R >> >> >>",
		stream("", "/X Do"),
		...forms,
	]);
}

describe("vellum > a document that asks for unbounded work", () => {
	it("stops a fan-out of forms once the operations run out", async () => {
		const vellum = new Vellum({ maxOperations: 1_000_000 });
		const pdf = fanOut(9);
		const started = performance.now();
		await expect(vellum.render(pdf)).rejects.toThrow(
			/more than 1000000 operations/,
		);
		await expect(vellum.extractText(pdf)).rejects.toThrow(
			/raise maxOperations/,
		);
		expect(performance.now() - started).toBeLessThan(10_000);
	});

	it("refuses renderAll before drawing pages that add up past the ceiling", async () => {
		const kids = Array.from(
			{ length: 40 },
			(_, page) => `${4 + page} 0 R`,
		).join(" ");
		const pdf = buildPdf([
			"<< /Type /Catalog /Pages 2 0 R >>",
			`<< /Type /Pages /Kids [${kids}] /Count 40 >>`,
			stream("", "0 0 7000 7000 re f"),
			...Array.from(
				{ length: 40 },
				() =>
					"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 7000 7000] /Contents 3 0 R >>",
			),
		]);
		const started = performance.now();
		await expect(new Vellum().renderAll(pdf)).rejects.toThrow(
			/raise maxTotalPixels/,
		);
		expect(performance.now() - started).toBeLessThan(2_000);
	});
});

describe("vellum > reading a large document", () => {
	const pages = 100_000;
	const kids = Array.from(
		{ length: pages },
		(_, page) => `${page + 3} 0 R`,
	).join(" ");
	const pdf = buildPdf([
		"<< /Type /Catalog /Pages 2 0 R >>",
		`<< /Type /Pages /Kids [${kids}] /Count ${pages} >>`,
		...Array.from(
			{ length: pages },
			() => "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] >>",
		),
	]);
	const vellum = new Vellum();

	/** How many times a 5 ms timer fires while `work` runs. */
	async function ticksDuring(work: () => Promise<unknown>): Promise<number> {
		let ticks = 0;
		const timer = setInterval(() => {
			ticks++;
		}, 5);
		try {
			await work();
		} finally {
			clearInterval(timer);
		}
		return ticks;
	}

	it("leaves the event loop free while it is parsed", async () => {
		expect(await ticksDuring(() => vellum.inspect(pdf))).toBeGreaterThan(0);
		expect(await ticksDuring(() => vellum.dimensions(pdf))).toBeGreaterThan(0);
		expect(await ticksDuring(() => vellum.metadata(pdf))).toBeGreaterThan(0);
		expect(await ticksDuring(() => vellum.formFields(pdf))).toBeGreaterThan(0);
	});

	it("still answers", async () => {
		expect(await vellum.pageCount(pdf)).toBe(pages);
		expect((await vellum.dimensions(pdf)).length).toBe(pages);
		expect(await vellum.formFields(pdf)).toEqual([]);
	});
});
