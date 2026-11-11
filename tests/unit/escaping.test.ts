import { inflateSync } from "node:zlib";
import { describe, expect, it } from "vitest";
import {
	A4,
	createBlank,
	PdfDocument,
	toBytes,
	Vellum,
} from "../../src/index.js";

const vellum = new Vellum();

/**
 * Text that closes the string it is written into and paints the page red,
 * then reopens one so the rest still parses: what a form value from a web
 * page can carry, if the escaping ever regressed.
 */
const INJECTION = ") Tj ET 1 0 0 rg 0 0 600 800 re f BT (";
const ESCAPED = "\\) Tj ET 1 0 0 rg 0 0 600 800 re f BT \\(";

/** Every stream in the file, inflated when it is Flate-encoded. */
function streams(pdf: Buffer): string[] {
	const out: string[] = [];
	const text = pdf.toString("latin1");
	const opener = /stream\r?\n/g;
	for (
		let match = opener.exec(text);
		match !== null;
		match = opener.exec(text)
	) {
		const start = match.index + match[0].length;
		const end = text.indexOf("endstream", start);
		if (end === -1) break;
		const raw = pdf.subarray(start, end);
		try {
			out.push(inflateSync(raw).toString("latin1"));
		} catch {
			out.push(raw.toString("latin1"));
		}
		opener.lastIndex = end;
	}
	return out;
}

function escapedIn(pdf: Buffer): boolean {
	return streams(pdf).some((stream) => stream.includes(ESCAPED));
}

async function bytesOf(doc: PdfDocument): Promise<Buffer> {
	const done = toBytes(doc);
	doc.end();
	return Buffer.from(await done);
}

/** A page with one text field, filled or not. */
async function oneField(value?: string): Promise<Buffer> {
	const doc = new PdfDocument({ size: "A4" });
	doc.initForm();
	doc.formText(
		"remarque",
		40,
		60,
		500,
		22,
		value === undefined ? {} : { value },
	);
	return bytesOf(doc);
}

describe("text written into a content stream", () => {
	it("is escaped by stampText, and reads back as given", async () => {
		const stamped = await vellum.stampText(createBlank([A4]), INJECTION, {
			x: 40,
			y: 80,
			size: 10,
		});
		expect(escapedIn(stamped)).toBe(true);
		await expect(vellum.extractText(stamped, { page: 1 })).resolves.toBe(
			INJECTION,
		);
	});

	it("is escaped by fillForm in the appearance it draws", async () => {
		const filled = await vellum.fillForm(await oneField(), {
			remarque: INJECTION,
		});
		expect(escapedIn(filled)).toBe(true);
		expect(await vellum.extractText(filled)).toContain(INJECTION.trim());
	});

	it("is escaped in a field's default value written by the document", async () => {
		const pdf = await oneField(INJECTION);
		expect(escapedIn(pdf)).toBe(true);
		expect(await vellum.extractText(pdf)).toContain(INJECTION.trim());
	});
});
