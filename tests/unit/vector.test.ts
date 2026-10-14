import { describe, expect, it } from "vitest";
import { PdfDocument, toBytes, Vellum, VellumError } from "../../src/index.js";
import { decodePng, type Pixels } from "./png.js";

const vellum = new Vellum();

/** A 200 x 200 page with nothing on it, to draw on. */
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
		(value, index) => Math.abs(value - (expected[index] ?? 0)) < 24,
	);
}

const WHITE: [number, number, number] = [255, 255, 255];
const RED: [number, number, number] = [255, 0, 0];

describe("PdfDocument — shapes", () => {
	it("fills a rectangle in a named colour and strokes a line", async () => {
		const doc = page();
		doc.rect(20, 20, 50, 50).fill("red");
		doc.moveTo(100, 150).lineTo(190, 150).lineWidth(10).stroke("#00f");
		const image = await pixels(doc);
		expect(near(image.at(45, 45), RED)).toBe(true);
		expect(near(image.at(10, 10), WHITE)).toBe(true);
		expect(near(image.at(150, 150), [0, 0, 255])).toBe(true);
		expect(near(image.at(150, 160), WHITE)).toBe(true);
	});

	it("draws circles, ellipses, rounded rectangles, polygons and arcs", async () => {
		const doc = page();
		doc.circle(50, 50, 30).fill([0, 128, 0]);
		doc.roundedRect(110, 20, 70, 60, 20).fill("blue");
		doc.polygon([20, 190], [80, 190], [50, 120]).fill("black");
		doc.arc(150, 150, 30, 0, Math.PI).lineWidth(6).stroke("red");
		const image = await pixels(doc);
		expect(near(image.at(50, 50), [0, 128, 0])).toBe(true);
		expect(near(image.at(25, 25), WHITE), "outside the circle").toBe(true);
		expect(near(image.at(145, 50), [0, 0, 255])).toBe(true);
		expect(near(image.at(111, 21), WHITE), "the corner is rounded off").toBe(
			true,
		);
		expect(near(image.at(50, 180), [0, 0, 0])).toBe(true);
		// The arc from 0 to π runs clockwise on the page: below its centre.
		expect(near(image.at(150, 180), RED)).toBe(true);
		expect(near(image.at(150, 120), WHITE)).toBe(true);
	});

	it("reads SVG path data, arcs included", async () => {
		const doc = page();
		doc.path("M20 20 h60 v60 h-60 Z").fill("red");
		doc.path("M120,100 a40,40 0 1,0 80,0 a40,40 0 1,0 -80,0").fill("blue");
		const image = await pixels(doc);
		expect(near(image.at(50, 50), RED)).toBe(true);
		expect(near(image.at(160, 100), [0, 0, 255])).toBe(true);
		expect(near(image.at(125, 65), WHITE)).toBe(true);
	});

	it("leaves the hole with even-odd", async () => {
		const doc = page();
		doc.rect(20, 20, 160, 160).rect(60, 60, 80, 80).fill("black", "even-odd");
		const image = await pixels(doc);
		expect(near(image.at(30, 100), [0, 0, 0])).toBe(true);
		expect(near(image.at(100, 100), WHITE)).toBe(true);
	});

	it("dashes a stroke", async () => {
		const doc = page();
		doc
			.moveTo(0, 100)
			.lineTo(200, 100)
			.lineWidth(8)
			.dash(20, { space: 20 })
			.stroke();
		const image = await pixels(doc);
		expect(near(image.at(10, 100), [0, 0, 0])).toBe(true);
		expect(near(image.at(30, 100), WHITE)).toBe(true);
		expect(() => page().dash(0)).toThrow(VellumError);
	});
});

describe("PdfDocument — transforms and clipping", () => {
	it("moves, rotates and scales what follows, until restore", async () => {
		const doc = page();
		doc.save().translate(100, 100).rect(0, 0, 20, 20).fill("red").restore();
		doc
			.save()
			.rotate(90, { origin: [50, 50] })
			.rect(50, 50, 40, 10)
			.fill("blue")
			.restore();
		doc.save().scale(2).rect(5, 80, 10, 10).fill("black").restore();
		doc.rect(0, 0, 10, 10).fill("red");
		const image = await pixels(doc);
		expect(near(image.at(110, 110), RED)).toBe(true);
		// 90° clockwise about (50, 50): the bar now hangs down from it.
		expect(near(image.at(45, 80), [0, 0, 255])).toBe(true);
		expect(near(image.at(80, 55), WHITE)).toBe(true);
		expect(near(image.at(20, 170), [0, 0, 0])).toBe(true);
		expect(near(image.at(5, 5), RED), "restored to the page's own space").toBe(
			true,
		);
	});

	it("clips to a path until restore", async () => {
		const doc = page();
		doc
			.save()
			.rect(0, 0, 100, 200)
			.clip()
			.rect(0, 0, 200, 200)
			.fill("red")
			.restore();
		doc.rect(150, 0, 50, 50).fill("blue");
		const image = await pixels(doc);
		expect(near(image.at(50, 100), RED)).toBe(true);
		expect(near(image.at(150, 100), WHITE)).toBe(true);
		expect(near(image.at(175, 25), [0, 0, 255]), "drawn after restore").toBe(
			true,
		);
	});

	it("refuses a restore without a save, where pdfkit writes a broken page", () => {
		expect(() => page().restore()).toThrow(
			expect.objectContaining({ code: "E_VELLUM_UNBALANCED_RESTORE" }),
		);
	});
});

describe("PdfDocument — colours, opacity and gradients", () => {
	it("paints in CMYK and in a spot colour", async () => {
		const doc = page();
		doc.rect(0, 0, 100, 100).fill([0, 100, 100, 0]);
		doc.addSpotColor("Corporate Blue", 100, 50, 0, 0);
		doc.rect(100, 0, 100, 100).fill("Corporate Blue");
		const image = await pixels(doc);
		const cmykRed = image.at(50, 50);
		expect(cmykRed[0]).toBeGreaterThan(200);
		expect(cmykRed[1]).toBeLessThan(80);
		const spot = image.at(150, 50);
		expect(spot[2]).toBeGreaterThan(spot[0]);
	});

	it("blends with opacity", async () => {
		const doc = page();
		doc.rect(0, 0, 100, 100).fillOpacity(0.5).fill("black");
		doc.rect(100, 0, 100, 100).fillColor("black", 1).fill();
		const image = await pixels(doc);
		const grey = image.at(50, 50)[0];
		expect(grey).toBeGreaterThan(100);
		expect(grey).toBeLessThan(160);
		expect(image.at(150, 50)[0]).toBeLessThan(30);
	});

	it("fills with linear and radial gradients", async () => {
		const doc = page();
		const linear = doc
			.linearGradient(0, 0, 200, 0)
			.stop(0, "red")
			.stop(1, "blue");
		doc.rect(0, 0, 200, 100).fill(linear);
		const radial = doc
			.radialGradient(100, 150, 0, 100, 150, 50)
			.stop(0, "white")
			.stop(1, "black");
		doc.rect(0, 100, 200, 100).fill(radial);
		const image = await pixels(doc);
		const left = image.at(5, 50);
		const right = image.at(195, 50);
		expect(left[0]).toBeGreaterThan(200);
		expect(right[2]).toBeGreaterThan(200);
		expect(image.at(100, 150)[0]).toBeGreaterThan(200);
		expect(image.at(100, 198)[0]).toBeLessThan(60);
	});

	it("draws text in the fill colour", async () => {
		const doc = page();
		doc.fillColor("red").fontSize(60).text("■", 10, 10, { lineBreak: false });
		const image = await pixels(doc);
		let red = 0;
		for (let x = 10; x < 80; x += 5) {
			for (let y = 10; y < 80; y += 5) if (near(image.at(x, y), RED)) red++;
		}
		expect(red).toBeGreaterThan(0);
	});

	it("refuses a colour it cannot read, where pdfkit keeps the last one", () => {
		const doc = page();
		expect(() => doc.fillColor("redd")).toThrow(
			expect.objectContaining({ code: "E_VELLUM_INVALID_COLOR" }),
		);
		expect(() => doc.fillColor([300, 0, 0])).toThrow(VellumError);
		expect(() =>
			doc
				.linearGradient(0, 0, 1, 0)
				.stop(0, "red")
				.stop(1, [0, 0, 0, 100])
				.handle(),
		).toThrow(VellumError);
	});
});

describe("PdfDocument — the rest of the vector surface", () => {
	it("strokes with caps, joins, a miter limit, curves and closed paths", async () => {
		const doc = page();
		doc
			.lineWidth(12)
			.lineCap("round")
			.lineJoin(2)
			.miterLimit(4)
			.moveTo(20, 20)
			.bezierCurveTo(60, 20, 60, 60, 100, 60)
			.quadraticCurveTo(140, 60, 180, 20)
			.closePath()
			.stroke("black");
		doc.lineCap(2).lineJoin("round").dash([4, 4]).undash();
		doc.moveTo(20, 150).lineTo(180, 150).stroke();
		const image = await pixels(doc);
		expect(near(image.at(100, 60), [0, 0, 0])).toBe(true);
		expect(near(image.at(100, 150), [0, 0, 0]), "undash() makes it solid").toBe(
			true,
		);
		expect(near(image.at(102, 150), [0, 0, 0])).toBe(true);
	});

	it("fills and strokes at once, in the forms pdfkit takes", async () => {
		const doc = page();
		doc.rect(10, 10, 60, 60).lineWidth(10).fillAndStroke("red", "blue");
		doc.rect(110, 10, 60, 60).fillAndStroke("even-odd");
		doc.rect(10, 110, 60, 60).fillAndStroke("red", "evenodd");
		doc.rect(110, 110, 60, 60).fill("non-zero");
		doc.fill();
		const image = await pixels(doc);
		expect(near(image.at(40, 40), RED)).toBe(true);
		expect(near(image.at(10, 40), [0, 0, 255]), "the stroke is blue").toBe(
			true,
		);
		expect(near(image.at(40, 140), RED)).toBe(true);
	});

	it("sets stroke opacity, both opacities, and a gradient's transform", async () => {
		const doc = page();
		doc
			.strokeColor("black", 0.5)
			.lineWidth(20)
			.moveTo(0, 50)
			.lineTo(200, 50)
			.stroke();
		doc.opacity(1).strokeOpacity(1);
		const shifted = doc
			.linearGradient(0, 0, 100, 0)
			.stop(0, "red")
			.stop(1, "blue")
			.setTransform(1, 0, 0, 1, 100, 0);
		doc.rect(0, 100, 200, 100).fill(shifted);
		doc
			.save()
			.scale(2, { origin: [100, 100] })
			.restore();
		doc
			.save()
			.scale(1, 2, { origin: [0, 0] })
			.restore();
		const image = await pixels(doc);
		const grey = image.at(100, 50)[0];
		expect(grey).toBeGreaterThan(100);
		expect(grey).toBeLessThan(160);
		// Shifted by 100: still red at x = 100, before the stops begin.
		expect(image.at(100, 150)[0]).toBeGreaterThan(200);
	});

	it("paints nothing when there is no path", async () => {
		const doc = page();
		doc.fill("red").stroke();
		const image = await pixels(doc);
		expect(near(image.at(100, 100), WHITE)).toBe(true);
	});
});

describe("PdfDocument — text decorations, transforms and links", () => {
	it("underlines and strikes through in the fill colour", async () => {
		const doc = page();
		doc
			.fillColor("red")
			.fontSize(20)
			.text("Souligné", 10, 10, { underline: true, strike: true });
		const lineHeight = doc.currentLineHeight();
		const image = await pixels(doc);
		// Centred on y + line height − thickness: 2 points at 20.
		const underline = Math.floor(10 + lineHeight - 2);
		expect(near(image.at(30, underline), RED)).toBe(true);
	});

	it("draws text rotated, oblique, stroked and scaled", async () => {
		const doc = page();
		doc.fontSize(16).text("Tourné", 100, 100, { rotation: 90 });
		doc.text("Penché", 10, 10, { oblique: true });
		doc.text("Penché aussi", 10, 40, { oblique: 20, horizontalScaling: 80 });
		doc.lineWidth(0.5).text("Contour", 10, 160, { stroke: true, fill: true });
		const bytes = toBytes(doc);
		doc.end();
		const text = await vellum.extractText(Buffer.from(await bytes));
		// Vertical text reads back a letter per line.
		const letters = text.replace(/\s+/g, "");
		for (const word of ["Tourné", "Penché", "Contour"])
			expect(letters).toContain(word);
	});

	it("links text and rectangles to URLs, pages and destinations", async () => {
		const doc = new PdfDocument({ compress: false });
		doc.text("Site", { link: "https://c9up.dev" });
		doc.text("Plus loin", { goTo: "fin" });
		doc.link(72, 300, 100, 20, "mailto:hello@example.com");
		doc.addPage();
		doc.text("Fin", { destination: "fin", link: 0 });
		doc.goTo(72, 200, 100, 20, "fin");
		doc.addNamedDestination("haut");
		doc.addNamedDestination("bas", "XYZ", 72, 700);
		const bytes = Buffer.from(
			await (async () => {
				const done = toBytes(doc);
				doc.end();
				return done;
			})(),
		);
		expect(bytes.includes(Buffer.from("https://c9up.dev"))).toBe(true);
		expect(bytes.includes(Buffer.from("mailto:hello@example.com"))).toBe(true);
		expect(bytes.includes(Buffer.from("/Dests"))).toBe(true);
	});

	it("refuses a link that would run code or open files", () => {
		const doc = page();
		for (const target of ["javascript:alert(1)", "file:///etc/passwd"]) {
			expect(() => doc.link(0, 0, 10, 10, target)).toThrow(
				expect.objectContaining({ code: "E_VELLUM_INVALID_LINK" }),
			);
			expect(() => doc.text("x", { link: target })).toThrow(VellumError);
		}
	});

	it("refuses a link to a page or a destination that never exists", () => {
		const doc = page();
		doc.link(0, 0, 10, 10, 5);
		expect(() => doc.end()).toThrow(
			expect.objectContaining({ code: "E_VELLUM_INVALID_LINK" }),
		);
		const other = page();
		other.goTo(0, 0, 10, 10, "nowhere");
		expect(() => other.end()).toThrow(VellumError);
	});
});

describe("PdfDocument — SVG and raw path content", () => {
	const SQUARE =
		'<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><rect width="10" height="10" fill="red"/><rect x="10" width="10" height="10" fill="blue"/></svg>';

	it("draws SVG as vectors, sized and fitted like an image", async () => {
		const doc = page();
		doc.svg(SQUARE, 0, 0);
		doc.svg(SQUARE, 0, 100, { width: 200 });
		doc.svg(Buffer.from(SQUARE), 100, 0, { fit: [100, 100], valign: "bottom" });
		const top = doc.y;
		doc.svg(SQUARE, { height: 5 });
		expect(doc.y - top).toBe(5);
		const image = await pixels(doc);
		expect(near(image.at(5, 5), RED)).toBe(true);
		expect(near(image.at(15, 5), [0, 0, 255])).toBe(true);
		// 200 wide keeps 2:1: red on the left half, 100 high.
		expect(near(image.at(50, 150), RED)).toBe(true);
		expect(near(image.at(150, 150), [0, 0, 255])).toBe(true);
		// Fitted 100 x 50 to the bottom of its 100 x 100 box.
		expect(near(image.at(120, 75), RED)).toBe(true);
		expect(near(image.at(120, 25), WHITE)).toBe(true);
	});

	it("refuses what is not an SVG", () => {
		expect(() => page().svg("<html></html>")).toThrow(
			expect.objectContaining({ code: "E_VELLUM_INVALID_SVG" }),
		);
	});

	it("takes PDF path operators through addContent, and nothing else", async () => {
		const doc = page();
		doc.save().translate(100, 100);
		doc.addContent("0 0 m 50 0 l 50 50 l h").fill("red");
		doc.addContent("-80 -80 30 30 re").fill("blue");
		doc
			.addContent("0 60 m 10 70 20 70 v 30 60 30 70 y 30 60 40 60 50 70 c h")
			.stroke();
		doc.restore();
		const image = await pixels(doc);
		expect(near(image.at(140, 110), RED)).toBe(true);
		expect(near(image.at(110, 140), WHITE), "below the diagonal").toBe(true);
		expect(near(image.at(35, 35), [0, 0, 255])).toBe(true);
		for (const content of [
			"BT /F1 12 Tf (x) Tj ET",
			"0 0 m 10 10 l S",
			"1 0 0 1 0 0 cm",
			"1 2 m 3",
		]) {
			expect(() => page().addContent(content), content).toThrow(
				expect.objectContaining({ code: "E_VELLUM_UNSUPPORTED_CONTENT" }),
			);
		}
	});
});
