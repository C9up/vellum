/**
 * Building paths the way pdfkit's vector methods describe them, before the
 * engine paints them.
 */

/** 4·(√2 − 1)/3: how far a cubic's handles reach to draw a quarter circle. */
const KAPPA = (4 * (Math.SQRT2 - 1)) / 3;

/** A path being built: verbs (`M L C Q Z`) and the numbers they take. */
export class PathRecorder {
	verbs = "";
	coords: number[] = [];
	#x = 0;
	#y = 0;
	#startX = 0;
	#startY = 0;
	/** The last cubic's second handle, for SVG's `S`. */
	#cubicHandle: [number, number] | undefined;
	/** The last quadratic's handle, for SVG's `T`. */
	#quadHandle: [number, number] | undefined;

	get point(): [number, number] {
		return [this.#x, this.#y];
	}

	moveTo(x: number, y: number): void {
		this.verbs += "M";
		this.coords.push(x, y);
		this.#x = this.#startX = x;
		this.#y = this.#startY = y;
		this.#cubicHandle = this.#quadHandle = undefined;
	}

	lineTo(x: number, y: number): void {
		this.verbs += "L";
		this.coords.push(x, y);
		this.#x = x;
		this.#y = y;
		this.#cubicHandle = this.#quadHandle = undefined;
	}

	bezierCurveTo(
		x1: number,
		y1: number,
		x2: number,
		y2: number,
		x: number,
		y: number,
	): void {
		this.verbs += "C";
		this.coords.push(x1, y1, x2, y2, x, y);
		this.#x = x;
		this.#y = y;
		this.#cubicHandle = [x2, y2];
		this.#quadHandle = undefined;
	}

	quadraticCurveTo(x1: number, y1: number, x: number, y: number): void {
		this.verbs += "Q";
		this.coords.push(x1, y1, x, y);
		this.#x = x;
		this.#y = y;
		this.#quadHandle = [x1, y1];
		this.#cubicHandle = undefined;
	}

	closePath(): void {
		this.verbs += "Z";
		this.#x = this.#startX;
		this.#y = this.#startY;
		this.#cubicHandle = this.#quadHandle = undefined;
	}

	/** The reflection of the last cubic handle, or the current point. */
	reflectedCubic(): [number, number] {
		const handle = this.#cubicHandle;
		return handle === undefined
			? [this.#x, this.#y]
			: [2 * this.#x - handle[0], 2 * this.#y - handle[1]];
	}

	reflectedQuad(): [number, number] {
		const handle = this.#quadHandle;
		return handle === undefined
			? [this.#x, this.#y]
			: [2 * this.#x - handle[0], 2 * this.#y - handle[1]];
	}

	/** Hand the path over and start an empty one, as painting does. */
	take(): { verbs: string; coords: number[] } {
		const taken = { verbs: this.verbs, coords: this.coords };
		this.verbs = "";
		this.coords = [];
		this.#cubicHandle = this.#quadHandle = undefined;
		return taken;
	}
}

export function rect(
	path: PathRecorder,
	x: number,
	y: number,
	width: number,
	height: number,
): void {
	path.moveTo(x, y);
	path.lineTo(x + width, y);
	path.lineTo(x + width, y + height);
	path.lineTo(x, y + height);
	path.closePath();
}

/** pdfkit's `roundedRect`: one radius, or `[topLeft, topRight, bottomRight, bottomLeft]`. */
export function roundedRect(
	path: PathRecorder,
	x: number,
	y: number,
	width: number,
	height: number,
	radius: number | readonly number[] = 0,
): void {
	const radii =
		typeof radius === "number" ? [radius, radius, radius, radius] : radius;
	const limit = Math.min(0.5 * width, 0.5 * height);
	const clamp = (value: number | undefined) =>
		Math.max(0, Math.min(value || 0, limit));
	const topLeft = clamp(radii[0]);
	const topRight = clamp(radii[1]);
	const bottomRight = clamp(radii[2]);
	const bottomLeft = clamp(radii[3]);
	const handle = (r: number) => r * (1 - KAPPA);
	path.moveTo(x + topLeft, y);
	path.lineTo(x + width - topRight, y);
	if (topRight > 0) {
		const c = handle(topRight);
		path.bezierCurveTo(
			x + width - c,
			y,
			x + width,
			y + c,
			x + width,
			y + topRight,
		);
	}
	path.lineTo(x + width, y + height - bottomRight);
	if (bottomRight > 0) {
		const c = handle(bottomRight);
		path.bezierCurveTo(
			x + width,
			y + height - c,
			x + width - c,
			y + height,
			x + width - bottomRight,
			y + height,
		);
	}
	path.lineTo(x + bottomLeft, y + height);
	if (bottomLeft > 0) {
		const c = handle(bottomLeft);
		path.bezierCurveTo(
			x + c,
			y + height,
			x,
			y + height - c,
			x,
			y + height - bottomLeft,
		);
	}
	path.lineTo(x, y + topLeft);
	if (topLeft > 0) {
		const c = handle(topLeft);
		path.bezierCurveTo(x, y + c, x + c, y, x + topLeft, y);
	}
	path.closePath();
}

export function ellipse(
	path: PathRecorder,
	cx: number,
	cy: number,
	rx: number,
	ry: number = rx,
): void {
	const x = cx - rx;
	const y = cy - ry;
	const ox = rx * KAPPA;
	const oy = ry * KAPPA;
	const xe = x + rx * 2;
	const ye = y + ry * 2;
	path.moveTo(x, cy);
	path.bezierCurveTo(x, cy - oy, cx - ox, y, cx, y);
	path.bezierCurveTo(cx + ox, y, xe, cy - oy, xe, cy);
	path.bezierCurveTo(xe, cy + oy, cx + ox, ye, cx, ye);
	path.bezierCurveTo(cx - ox, ye, x, cy + oy, x, cy);
	path.closePath();
}

/** pdfkit's `arc`: angles in radians, clockwise on the page unless `anticlockwise`. */
export function arc(
	path: PathRecorder,
	x: number,
	y: number,
	radius: number,
	startAngle: number,
	endAngle: number,
	anticlockwise = false,
): void {
	const turn = 2 * Math.PI;
	let delta = endAngle - startAngle;
	if (Math.abs(delta) > turn) {
		delta = turn;
	} else if (delta !== 0 && anticlockwise !== delta < 0) {
		delta = (anticlockwise ? -1 : 1) * turn + delta;
	}
	const segments = Math.max(1, Math.ceil(Math.abs(delta) / (Math.PI / 2)));
	const step = delta / segments;
	const handle = (step / (Math.PI / 2)) * KAPPA * radius;
	let angle = startAngle;
	let ax = x + Math.cos(angle) * radius;
	let ay = y + Math.sin(angle) * radius;
	let dx = -Math.sin(angle) * handle;
	let dy = Math.cos(angle) * handle;
	path.moveTo(ax, ay);
	for (let index = 0; index < segments; index++) {
		const c1x = ax + dx;
		const c1y = ay + dy;
		angle += step;
		ax = x + Math.cos(angle) * radius;
		ay = y + Math.sin(angle) * radius;
		dx = -Math.sin(angle) * handle;
		dy = Math.cos(angle) * handle;
		path.bezierCurveTo(c1x, c1y, ax - dx, ay - dy, ax, ay);
	}
}

export function polygon(
	path: PathRecorder,
	points: ReadonlyArray<readonly [number, number]>,
): void {
	const [first, ...rest] = points;
	if (first === undefined) return;
	path.moveTo(first[0], first[1]);
	for (const [x, y] of rest) path.lineTo(x, y);
	path.closePath();
}

const ARGUMENTS: Readonly<Record<string, number>> = {
	M: 2,
	L: 2,
	H: 1,
	V: 1,
	C: 6,
	S: 4,
	Q: 4,
	T: 2,
	A: 7,
	Z: 0,
};

const NUMBER = /^[+-]?(?:\d+\.?\d*|\.\d+)(?:[eE][+-]?\d+)?/;

/**
 * SVG path data onto `path`. As SVG renders a path up to its first error,
 * whatever follows a malformed command is dropped.
 */
export function svgPath(path: PathRecorder, data: string): void {
	let rest = data;
	let command: string | undefined;
	let first = true;
	const skip = () => {
		rest = rest.replace(/^[\s,]+/, "");
	};
	for (;;) {
		skip();
		if (rest.length === 0) return;
		const head = rest.charAt(0);
		if (/[a-zA-Z]/.test(head)) {
			if (ARGUMENTS[head.toUpperCase()] === undefined) return;
			command = head;
			rest = rest.slice(1);
		} else if (command === undefined) {
			return;
		}
		if (first && command.toUpperCase() !== "M") return;
		first = false;
		const upper = command.toUpperCase();
		const count = ARGUMENTS[upper] ?? 0;
		const args: number[] = [];
		for (let index = 0; index < count; index++) {
			skip();
			// Arc flags may be written without a separator: `a1 1 0 01 1 1`.
			const isFlag = upper === "A" && (index === 3 || index === 4);
			const match = isFlag ? /^[01]/.exec(rest) : NUMBER.exec(rest);
			if (match === null) return;
			args.push(Number(match[0]));
			rest = rest.slice(match[0].length);
		}
		apply(path, command, args);
		// After a move, further pairs are lines.
		if (upper === "M") command = command === "m" ? "l" : "L";
		if (upper === "Z") command = undefined;
	}
}

function apply(path: PathRecorder, command: string, args: number[]): void {
	const relative = command === command.toLowerCase();
	const [cx, cy] = path.point;
	const ox = relative ? cx : 0;
	const oy = relative ? cy : 0;
	const at = (index: number) => args[index] ?? 0;
	switch (command.toUpperCase()) {
		case "M":
			path.moveTo(ox + at(0), oy + at(1));
			return;
		case "L":
			path.lineTo(ox + at(0), oy + at(1));
			return;
		case "H":
			path.lineTo(ox + at(0), cy);
			return;
		case "V":
			path.lineTo(cx, oy + at(0));
			return;
		case "C":
			path.bezierCurveTo(
				ox + at(0),
				oy + at(1),
				ox + at(2),
				oy + at(3),
				ox + at(4),
				oy + at(5),
			);
			return;
		case "S": {
			const [x1, y1] = path.reflectedCubic();
			path.bezierCurveTo(
				x1,
				y1,
				ox + at(0),
				oy + at(1),
				ox + at(2),
				oy + at(3),
			);
			return;
		}
		case "Q":
			path.quadraticCurveTo(ox + at(0), oy + at(1), ox + at(2), oy + at(3));
			return;
		case "T": {
			const [x1, y1] = path.reflectedQuad();
			path.quadraticCurveTo(x1, y1, ox + at(0), oy + at(1));
			return;
		}
		case "A":
			svgArc(
				path,
				cx,
				cy,
				at(0),
				at(1),
				at(2),
				at(3) !== 0,
				at(4) !== 0,
				ox + at(5),
				oy + at(6),
			);
			return;
		case "Z":
			path.closePath();
			return;
	}
}

/**
 * An elliptical arc as cubics, through the centre parameterisation of the
 * SVG specification (implementation notes, F.6.5 and F.6.6).
 */
function svgArc(
	path: PathRecorder,
	x1: number,
	y1: number,
	radiusX: number,
	radiusY: number,
	rotationDegrees: number,
	largeArc: boolean,
	sweep: boolean,
	x2: number,
	y2: number,
): void {
	let rx = Math.abs(radiusX);
	let ry = Math.abs(radiusY);
	if ((x1 === x2 && y1 === y2) || rx === 0 || ry === 0) {
		if (x1 !== x2 || y1 !== y2) path.lineTo(x2, y2);
		return;
	}
	const phi = (rotationDegrees * Math.PI) / 180;
	const cos = Math.cos(phi);
	const sin = Math.sin(phi);
	const dx = (x1 - x2) / 2;
	const dy = (y1 - y2) / 2;
	const x1p = cos * dx + sin * dy;
	const y1p = -sin * dx + cos * dy;
	const lambda = (x1p * x1p) / (rx * rx) + (y1p * y1p) / (ry * ry);
	if (lambda > 1) {
		const scale = Math.sqrt(lambda);
		rx *= scale;
		ry *= scale;
	}
	const numerator =
		rx * rx * ry * ry - rx * rx * y1p * y1p - ry * ry * x1p * x1p;
	const denominator = rx * rx * y1p * y1p + ry * ry * x1p * x1p;
	const factor =
		(largeArc === sweep ? -1 : 1) *
		Math.sqrt(Math.max(0, numerator / denominator));
	const cxp = (factor * rx * y1p) / ry;
	const cyp = (-factor * ry * x1p) / rx;
	const centreX = cos * cxp - sin * cyp + (x1 + x2) / 2;
	const centreY = sin * cxp + cos * cyp + (y1 + y2) / 2;
	const angle = (ux: number, uy: number, vx: number, vy: number) => {
		const sign = ux * vy - uy * vx < 0 ? -1 : 1;
		const dot = ux * vx + uy * vy;
		const length = Math.hypot(ux, uy) * Math.hypot(vx, vy);
		return sign * Math.acos(Math.min(1, Math.max(-1, dot / length)));
	};
	const theta = angle(1, 0, (x1p - cxp) / rx, (y1p - cyp) / ry);
	let delta = angle(
		(x1p - cxp) / rx,
		(y1p - cyp) / ry,
		(-x1p - cxp) / rx,
		(-y1p - cyp) / ry,
	);
	if (!sweep && delta > 0) delta -= 2 * Math.PI;
	if (sweep && delta < 0) delta += 2 * Math.PI;

	const segments = Math.max(1, Math.ceil(Math.abs(delta) / (Math.PI / 2)));
	const step = delta / segments;
	const handle = (4 / 3) * Math.tan(step / 4);
	const point = (t: number): [number, number] => [
		centreX + rx * Math.cos(t) * cos - ry * Math.sin(t) * sin,
		centreY + rx * Math.cos(t) * sin + ry * Math.sin(t) * cos,
	];
	const derivative = (t: number): [number, number] => [
		-rx * Math.sin(t) * cos - ry * Math.cos(t) * sin,
		-rx * Math.sin(t) * sin + ry * Math.cos(t) * cos,
	];
	let t = theta;
	for (let index = 0; index < segments; index++) {
		const [ax, ay] = point(t);
		const [adx, ady] = derivative(t);
		const next = t + step;
		const [bx, by] = index === segments - 1 ? [x2, y2] : point(next);
		const [bdx, bdy] = derivative(next);
		path.bezierCurveTo(
			ax + handle * adx,
			ay + handle * ady,
			bx - handle * bdx,
			by - handle * bdy,
			bx,
			by,
		);
		t = next;
	}
}
