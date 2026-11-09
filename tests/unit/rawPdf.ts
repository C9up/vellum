/**
 * Hand-assembled PDFs, for the documents no producer would write: nesting
 * a million levels deep, a stream that inflates a thousandfold, a form tree
 * that never ends. Each object body is written as given, numbered from 1, and
 * the xref table is computed so that every reader finds them.
 */
import { deflateSync } from "node:zlib";

export type ObjectBody = string | Buffer;

export function buildPdf(objects: ObjectBody[]): Buffer {
	const parts: Buffer[] = [
		Buffer.from("%PDF-1.7\n%\xe2\xe3\xcf\xd3\n", "latin1"),
	];
	let offset = parts[0]?.length ?? 0;
	const offsets: number[] = [];
	objects.forEach((body, index) => {
		offsets.push(offset);
		const head = Buffer.from(`${index + 1} 0 obj\n`, "latin1");
		const content =
			typeof body === "string" ? Buffer.from(body, "latin1") : body;
		const tail = Buffer.from("\nendobj\n", "latin1");
		parts.push(head, content, tail);
		offset += head.length + content.length + tail.length;
	});
	let xref = `xref\n0 ${objects.length + 1}\n0000000000 65535 f \n`;
	for (const at of offsets)
		xref += `${String(at).padStart(10, "0")} 00000 n \n`;
	xref += `trailer\n<< /Size ${objects.length + 1} /Root 1 0 R >>\nstartxref\n${offset}\n%%EOF\n`;
	parts.push(Buffer.from(xref, "latin1"));
	return Buffer.concat(parts);
}

/** A stream object, Flate-compressed when asked. */
export function stream(
	dictionary: string,
	data: Buffer | string,
	flate = false,
): Buffer {
	const raw = typeof data === "string" ? Buffer.from(data, "latin1") : data;
	const body = flate ? deflateSync(raw, { level: 9 }) : raw;
	const filter = flate ? "/Filter /FlateDecode " : "";
	return Buffer.concat([
		Buffer.from(
			`<< ${dictionary} ${filter}/Length ${body.length} >>\nstream\n`,
			"latin1",
		),
		body,
		Buffer.from("\nendstream", "latin1"),
	]);
}

/** One A4 page drawing object 4, with `resources` in its /Resources. */
export function onePage(
	content: Buffer,
	resources = "",
	extra: ObjectBody[] = [],
): Buffer {
	return buildPdf([
		"<< /Type /Catalog /Pages 2 0 R >>",
		"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
		`<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Contents 4 0 R /Resources << ${resources} >> >>`,
		content,
		...extra,
	]);
}

export const HELVETICA =
	"/Font << /F1 << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> >>";
