import { describe, expect, it } from "vitest";
import {
	arc,
	PathRecorder,
	polygon,
	svgPath,
} from "../../src/document/vector.js";

function parse(data: string): { verbs: string; coords: number[] } {
	const path = new PathRecorder();
	svgPath(path, data);
	return path.take();
}

const rounded = (values: number[]) =>
	// `|| 0` folds -0 into 0, which toEqual tells apart.
	values.map((value) => Math.round(value * 1000) / 1000 || 0);

describe("SVG path data", () => {
	it("reads absolute and relative lines, and pairs after a move as lines", () => {
		expect(parse("M10 10 L 50 10 l0 40 H10 v-40 Z")).toEqual({
			verbs: "MLLLLZ",
			coords: [10, 10, 50, 10, 50, 50, 10, 50, 10, 10],
		});
		expect(parse("m 5,5 10,0 0,10")).toEqual({
			verbs: "MLL",
			coords: [5, 5, 15, 5, 15, 15],
		});
	});

	it("reflects the previous handle for S and T", () => {
		const cubic = parse("M0 0 C 10 0 20 10 20 20 S 30 40 40 40");
		expect(cubic.verbs).toBe("MCC");
		// The reflection of (20, 10) about (20, 20) is (20, 30).
		expect(cubic.coords.slice(8, 10)).toEqual([20, 30]);
		const quad = parse("M0 0 Q 10 10 20 0 t 20 0");
		expect(quad.verbs).toBe("MQQ");
		expect(quad.coords.slice(6, 8)).toEqual([30, -10]);
		// With no curve before, the handle is the current point.
		expect(parse("M5 5 S 10 10 20 20").coords.slice(2, 4)).toEqual([5, 5]);
		expect(parse("M5 5 T 20 20").coords.slice(2, 4)).toEqual([5, 5]);
		expect(parse("M0 0 c 1 1 2 2 3 3 q 1 1 2 2").verbs).toBe("MCQ");
	});

	it("turns an arc into curves that end where the arc ends", () => {
		const half = parse("M0 0 A 10 10 0 0 1 20 0");
		expect(half.verbs).toBe("MCC");
		expect(rounded(half.coords.slice(-2))).toEqual([20, 0]);
		// Flags written without separators, and a radius too small to reach.
		const scaled = parse("M0 0 a1 1 0 01 30 0");
		expect(rounded(scaled.coords.slice(-2))).toEqual([30, 0]);
		const other = parse("M0 0 A 10 10 0 1 0 20 0");
		expect(other.verbs.length).toBeGreaterThan(2);
		// A zero radius is a straight line; an arc to its own start is nothing.
		expect(parse("M0 0 A 0 10 0 0 1 20 0")).toEqual({
			verbs: "ML",
			coords: [0, 0, 20, 0],
		});
		expect(parse("M0 0 A 10 10 0 0 1 0 0")).toEqual({
			verbs: "M",
			coords: [0, 0],
		});
	});

	it("draws up to the first error, as SVG renders", () => {
		expect(parse("L 10 10")).toEqual({ verbs: "", coords: [] });
		expect(parse("10 10")).toEqual({ verbs: "", coords: [] });
		expect(parse("M0 0 L10 10 X 5 5").verbs).toBe("ML");
		expect(parse("M0 0 L10").verbs).toBe("M");
	});
});

describe("arcs and polygons", () => {
	it("draws a full turn when asked for more", () => {
		const path = new PathRecorder();
		arc(path, 0, 0, 10, 0, 10 * Math.PI);
		expect(path.take().verbs).toBe("MCCCC");
	});

	it("goes the other way round when anticlockwise", () => {
		const path = new PathRecorder();
		arc(path, 0, 0, 10, 0, Math.PI / 2, true);
		const { verbs, coords } = path.take();
		// Three quarters, the long way round.
		expect(verbs).toBe("MCCC");
		expect(rounded(coords.slice(-2))).toEqual([0, 10]);
	});

	it("draws nothing for a polygon without points", () => {
		const path = new PathRecorder();
		polygon(path, []);
		expect(path.take().verbs).toBe("");
	});
});
