/**
 * pdfkit's tables, on the document's own API: styles merged from the table's
 * default, the column, the row and the cell; `*` columns sharing what is
 * left; `auto` rows as tall as their tallest cell; a row that does not fit
 * moving whole to the next page.
 *
 * One addition, which applications otherwise build by hand: `headerRows`
 * repeats the first rows at the top of every page the table runs onto.
 */

import type { ColourValue } from "./colours.js";
import type { FontSource, MarkOptions, TextOptions } from "./PdfDocument.js";
import type { Side, SideDefinition, Sides, Size } from "./sizes.js";
import type {
	PdfStructureContent,
	PdfStructureElement,
	StructureOptions,
} from "./structure.js";

/** The parts of the document a table draws with. */
export interface TableHost {
	x: number;
	y: number;
	readonly page: {
		readonly width: number;
		readonly height: number;
		readonly margins: Readonly<Sides<number>>;
		readonly contentHeight: number;
		maxY(): number;
	};
	sizeToPoint(
		size: Size | boolean | undefined,
		fallback?: Size | boolean,
		page?: { width: number; height: number },
		percentageOf?: number,
	): number;
	boundsOfString(
		text: unknown,
		x?: number,
		y?: number,
		options?: TextOptions,
	): { x: number; y: number; width: number; height: number };
	text(text: unknown, x?: number, y?: number, options?: TextOptions): unknown;
	save(): unknown;
	restore(): unknown;
	rect(x: number, y: number, width: number, height: number): unknown;
	fill(): unknown;
	stroke(): unknown;
	clip(): unknown;
	moveTo(x: number, y: number): unknown;
	lineTo(x: number, y: number): unknown;
	fillColor(colour: ColourValue): unknown;
	strokeColor(colour: ColourValue): unknown;
	lineWidth(width: number): unknown;
	dash(length: number, options?: { space?: number }): unknown;
	strokeOpacity(opacity: number): unknown;
	continueOnNewPage(): unknown;
	/** The font in use, to put back after a cell drew in its own. */
	fontState(): FontState;
	useFont(state: TableFont): void;
	struct(type: string, options?: StructureOptions): PdfStructureElement;
	markContent(tag: string, options?: MarkOptions): unknown;
	markStructureContent(tag: string): PdfStructureContent;
	endMarkedContent(): unknown;
}

export interface FontState {
	src: FontSource;
	family: string | undefined;
	size: number;
}

export interface TableFont {
	src?: FontSource;
	family?: string;
	size?: Size;
}

export type TableAlign =
	| "left"
	| "center"
	| "right"
	| "justify"
	| {
			x?: "left" | "center" | "right" | "justify";
			y?: "top" | "center" | "bottom";
	  };

/** How a cell looks; a column, a row and the table's default take the same. */
export interface CellStyle {
	padding?: SideDefinition<Side>;
	/** Line widths, 0 for none. */
	border?: SideDefinition<Side>;
	/**
	 * A colour for every side, or one per side. An array of numbers is one
	 * colour (`[255, 0, 0]`); pdfkit reads `[255, 0, 0]` as three sides.
	 */
	borderColor?: ColourValue | SideDefinition<string>;
	backgroundColor?: ColourValue;
	align?: TableAlign;
	textColor?: ColourValue;
	textStroke?: Size | boolean;
	textStrokeColor?: ColourValue;
	font?: TableFont;
	textOptions?: TextOptions;
	type?: "TH" | "TD" | "th" | "td";
	scope?: "Row" | "Column" | "Both" | "row" | "column" | "both";
	title?: string;
	id?: string;
	debug?: boolean;
}

export interface Cell extends CellStyle {
	text?: unknown;
	rowSpan?: number;
	colSpan?: number;
}

export type TableCell = Cell | string | number | boolean | null | undefined;

export interface ColumnStyle extends CellStyle {
	/** A size, a percentage of the table's width, or `*` to share what is left. */
	width?: Size | "*";
	minWidth?: Size;
	maxWidth?: Size;
}

export interface RowStyle extends CellStyle {
	/** A size, a percentage of the page's content height, or `auto`. */
	height?: Size | "auto";
	minHeight?: Size;
	maxHeight?: Size;
}

type Styles<T> =
	| T
	| Size
	| ReadonlyArray<T | Size | undefined>
	| ((index: number) => T | Size | undefined);

export interface TableOptions {
	position?: { x?: Size; y?: Size };
	maxWidth?: Size;
	columnStyles?: Styles<ColumnStyle>;
	rowStyles?: Styles<RowStyle>;
	defaultStyle?: ColumnStyle & RowStyle;
	data?: Iterable<Iterable<TableCell>>;
	/** The first rows, repeated at the top of each page the table runs onto. */
	headerRows?: number;
	/** Tag the table: a `Table` of `TR`s of `TH` and `TD` cells, added to this element. */
	structParent?: PdfStructureElement;
	id?: string;
	debug?: boolean;
}

interface ResolvedCell {
	text: string | undefined;
	rowIndex: number;
	colIndex: number;
	rowSpan: number;
	colSpan: number;
	padding: Sides<number>;
	border: Sides<number>;
	borderColor: Sides<ColourValue>;
	backgroundColor: ColourValue | undefined;
	align: {
		x: "left" | "center" | "right" | "justify";
		y: "top" | "center" | "bottom";
	};
	textColor: ColourValue;
	textStroke: number;
	textStrokeColor: ColourValue;
	font: TableFont;
	customFont: boolean;
	textOptions: TextOptions;
	debug: boolean;
	type: "TH" | "TD";
	scope: "Row" | "Column" | "Both" | undefined;
}

interface PlacedCell extends ResolvedCell {
	x: number;
	y: number;
	width: number;
	height: number;
	textX: number;
	textY: number;
	textAllocatedWidth: number;
	textAllocatedHeight: number;
	textBounds: { x: number; y: number; width: number; height: number };
	options: TextOptions;
}

interface ResolvedColumn {
	width: number | "*";
	minWidth: number;
	maxWidth: number;
	style: CellStyle;
}

interface ResolvedRow {
	height: number | "auto";
	minHeight: number;
	maxHeight: number;
	style: CellStyle;
}

const SIDES = ["top", "right", "bottom", "left"] as const;

function isTuple(value: unknown): value is readonly unknown[] {
	return Array.isArray(value);
}

/** One level's sides, each `undefined` where the level says nothing. */
function sidesOf<T>(
	definition: unknown,
	isValue: (value: unknown) => value is T,
): Partial<Sides<T>> {
	if (definition === undefined || definition === null) return {};
	if (isValue(definition)) {
		return {
			top: definition,
			right: definition,
			bottom: definition,
			left: definition,
		};
	}
	if (isTuple(definition)) {
		const [a, b, c, d] = definition;
		const pick = (value: unknown) => (isValue(value) ? value : undefined);
		return definition.length === 2
			? { top: pick(a), right: pick(b), bottom: pick(a), left: pick(b) }
			: { top: pick(a), right: pick(b), bottom: pick(c), left: pick(d) };
	}
	if (typeof definition === "object") {
		const read = (key: string) => {
			const value: unknown = Reflect.get(definition, key);
			return isValue(value) ? value : undefined;
		};
		if ("vertical" in definition || "horizontal" in definition) {
			return {
				top: read("vertical"),
				right: read("horizontal"),
				bottom: read("vertical"),
				left: read("horizontal"),
			};
		}
		return {
			top: read("top"),
			right: read("right"),
			bottom: read("bottom"),
			left: read("left"),
		};
	}
	return {};
}

function isSide(value: unknown): value is Side {
	return (
		typeof value === "number" ||
		typeof value === "string" ||
		typeof value === "boolean"
	);
}

function isColour(value: unknown): value is ColourValue {
	return (
		typeof value === "string" ||
		(isTuple(value) &&
			(value.length === 3 || value.length === 4) &&
			value.every((part) => typeof part === "number"))
	);
}

function mergeSides<T>(
	levels: ReadonlyArray<Partial<Sides<T>>>,
	fallback: T,
): Sides<T> {
	const out: Sides<T> = {
		top: fallback,
		right: fallback,
		bottom: fallback,
		left: fallback,
	};
	for (const side of SIDES) {
		for (const level of levels) {
			const value = level[side];
			if (value !== undefined) out[side] = value;
		}
	}
	return out;
}

function alignOf(align: TableAlign | undefined): { x?: string; y?: string } {
	if (align === undefined) return {};
	return typeof align === "string" ? { x: align, y: align } : align;
}

function last<T>(values: ReadonlyArray<T | undefined>): T | undefined {
	let found: T | undefined;
	for (const value of values) if (value !== undefined) found = value;
	return found;
}

function scopeOf(value: string | undefined): ResolvedCell["scope"] {
	switch (value?.toLowerCase()) {
		case "row":
			return "Row";
		case "column":
			return "Column";
		case "both":
			return "Both";
		default:
			return undefined;
	}
}

function horizontal(value: string | undefined): ResolvedCell["align"]["x"] {
	return value === "center" || value === "right" || value === "justify"
		? value
		: "left";
}

function vertical(value: string | undefined): ResolvedCell["align"]["y"] {
	return value === "center" || value === "bottom" ? value : "top";
}

function styleGetter<T extends object>(
	styles: Styles<T> | undefined,
): (index: number) => T | Size | undefined {
	if (styles === undefined) return () => undefined;
	if (typeof styles === "function") {
		const cache = new Map<number, T | Size | undefined>();
		return (index) => {
			if (!cache.has(index)) cache.set(index, styles(index));
			return cache.get(index);
		};
	}
	if (isTuple(styles)) return (index) => styles[index];
	// pdfkit takes a bare size only from an array or a function, and
	// silently ignores `rowStyles: 30`; here it is every row's height.
	return () => styles;
}

/** pdfkit's `computeBounds`: the box text rotated by `rotation` fits in. */
function rotatedBounds(
	rotation: number,
	width: number,
	height: number,
): { width: number; height: number } {
	const radians = (rotation * Math.PI) / 180;
	const cos = rotation % 180 === 90 ? 0 : Math.cos(radians);
	const sin = rotation % 180 === 0 ? 0 : Math.sin(radians);
	let maxWidth: number;
	let maxHeight: number;
	const straight = rotation === 0 || rotation === 180;
	const sideways = rotation === 90 || rotation === 270;
	const firstQuadrant = rotation < 90 || (rotation > 180 && rotation < 270);
	if (straight) {
		maxWidth = width;
		maxHeight = height;
	} else if (sideways) {
		maxWidth = height;
		maxHeight = width;
	} else if (firstQuadrant) {
		maxWidth = width / (2 * cos);
		maxHeight = width / (2 * sin);
	} else {
		maxHeight = width / (2 * cos);
		maxWidth = width / (2 * sin);
	}
	if (sin * maxWidth + cos * maxHeight > height && !straight && !sideways) {
		const denominator = cos * cos - sin * sin;
		if (firstQuadrant) {
			maxWidth = (width * cos - height * sin) / denominator;
			maxHeight = (height * cos - width * sin) / denominator;
		} else {
			maxHeight = (width * cos - height * sin) / denominator;
			maxWidth = (height * cos - width * sin) / denominator;
		}
	}
	return { width: Math.abs(maxWidth), height: Math.abs(maxHeight) };
}

export class PdfTable<Document> {
	readonly #host: TableHost;
	readonly #document: Document;
	readonly #options: TableOptions;
	readonly #position: { x: number; y: number };
	readonly #maxWidth: number;
	readonly #columnStyle: (index: number) => ResolvedColumn;
	readonly #rowStyle: (index: number) => ResolvedRow;
	readonly #claimed = new Set<string>();
	#rowIndex = 0;
	#ended = false;
	#columnWidths: number[] = [];
	#columnX: number[] = [];
	#rowHeights: number[] = [];
	#rowY: number[] = [];
	#buffer = new Set<ResolvedCell>();
	/** The header rows as first drawn, to draw again on each new page. */
	readonly #header: PlacedCell[] = [];
	#headerTop = 0;
	#headerHeight = 0;
	/** The `Table` element, when the table is tagged. */
	readonly #structure: PdfStructureElement | undefined;

	constructor(host: TableHost, document: Document, options: TableOptions = {}) {
		this.#host = host;
		this.#document = document;
		this.#options = options;
		this.#position = {
			x: host.sizeToPoint(options.position?.x, host.x),
			y: host.sizeToPoint(options.position?.y, host.y),
		};
		this.#maxWidth = host.sizeToPoint(
			options.maxWidth,
			host.page.width - host.page.margins.right - this.#position.x,
		);
		const column = styleGetter(options.columnStyles);
		const row = styleGetter(options.rowStyles);
		const defaults = options.defaultStyle ?? {};
		this.#columnStyle = (index) => this.#column(defaults, column(index));
		this.#rowStyle = (index) => this.#row(defaults, row(index));
		if (options.structParent !== undefined) {
			this.#structure = host.struct("Table");
			options.structParent.add(this.#structure);
		}
		if (options.data !== undefined) {
			for (const cells of options.data) this.row(cells);
			this.end();
		}
	}

	#column(
		defaults: ColumnStyle,
		given: ColumnStyle | Size | undefined,
	): ResolvedColumn {
		const style: ColumnStyle =
			typeof given === "object" ? given : { width: given };
		const host = this.#host;
		const width = style.width ?? defaults.width;
		const toPoints = (size: Size | undefined) =>
			host.sizeToPoint(size, 0, host.page, this.#maxWidth);
		return {
			width: width === undefined || width === "*" ? "*" : toPoints(width),
			minWidth: toPoints(style.minWidth ?? defaults.minWidth),
			maxWidth: toPoints(style.maxWidth ?? defaults.maxWidth),
			style,
		};
	}

	#row(defaults: RowStyle, given: RowStyle | Size | undefined): ResolvedRow {
		const style: RowStyle =
			typeof given === "object" ? given : { height: given };
		const host = this.#host;
		const height = style.height ?? defaults.height;
		const toPoints = (size: Size | undefined) =>
			host.sizeToPoint(size, 0, host.page, host.page.contentHeight);
		return {
			height:
				height === undefined || height === "auto" ? "auto" : toPoints(height),
			minHeight: toPoints(style.minHeight ?? defaults.minHeight),
			maxHeight: toPoints(style.maxHeight ?? defaults.maxHeight),
			style,
		};
	}

	#resolve(given: TableCell, rowIndex: number, colIndex: number): ResolvedCell {
		const cell: Cell =
			typeof given === "object" && given !== null ? given : { text: given };
		const levels: CellStyle[] = [
			this.#options.defaultStyle ?? {},
			this.#columnStyle(colIndex).style,
			this.#rowStyle(rowIndex).style,
			cell,
		];
		const host = this.#host;
		const font: TableFont = {};
		for (const level of [levels[1]?.font, levels[2]?.font, cell.font]) {
			if (level === undefined) continue;
			if (level.src !== undefined) {
				font.src = level.src;
				font.family = level.family;
			} else if (level.family !== undefined) {
				font.family = level.family;
			}
			if (level.size !== undefined) font.size = level.size;
		}
		const customFont = Object.values(font).some((value) => value !== undefined);
		const saved = host.fontState();
		if (customFont) host.useFont(font);
		const points = (fallback: Size) => (value: Side) =>
			host.sizeToPoint(value, fallback);
		const padding = mergeSides(
			levels.map((level) => sidesOf(level.padding, isSide)),
			"0.25em",
		);
		const border = mergeSides(
			levels.map((level) => sidesOf(level.border, isSide)),
			1,
		);
		const borderColor = mergeSides<ColourValue>(
			levels.map((level) => sidesOf(level.borderColor, isColour)),
			"black",
		);
		const align = levels.map((level) => alignOf(level.align));
		const textOptions: TextOptions = {};
		for (const level of levels) Object.assign(textOptions, level.textOptions);
		const resolved: ResolvedCell = {
			text:
				cell.text === undefined || cell.text === null
					? undefined
					: String(cell.text),
			rowIndex,
			colIndex,
			rowSpan: Math.max(1, cell.rowSpan ?? 1),
			colSpan: Math.max(1, cell.colSpan ?? 1),
			padding: {
				top: points("0.25em")(padding.top),
				right: points("0.25em")(padding.right),
				bottom: points("0.25em")(padding.bottom),
				left: points("0.25em")(padding.left),
			},
			border: {
				top: points(1)(border.top),
				right: points(1)(border.right),
				bottom: points(1)(border.bottom),
				left: points(1)(border.left),
			},
			borderColor,
			backgroundColor: last(levels.map((level) => level.backgroundColor)),
			align: {
				x: horizontal(last(align.map((level) => level.x))),
				y: vertical(last(align.map((level) => level.y))),
			},
			textColor: last(levels.map((level) => level.textColor)) ?? "black",
			textStroke: host.sizeToPoint(
				last(levels.map((level) => level.textStroke)),
				0,
			),
			textStrokeColor:
				last(levels.map((level) => level.textStrokeColor)) ?? "black",
			font,
			customFont,
			textOptions,
			debug:
				this.#options.debug ??
				last(levels.map((level) => level.debug)) ??
				false,
			type:
				last(levels.map((level) => level.type))?.toUpperCase() === "TH"
					? "TH"
					: "TD",
			scope: scopeOf(last(levels.map((level) => level.scope))),
		};
		if (customFont) host.useFont(saved);
		return resolved;
	}

	/** Draw a row. The table answers, or the document once `lastRow` ends it. */
	row(cells: Iterable<TableCell>, lastRow = false): this {
		if (this.#ended) {
			throw new Error(`Table was marked as ended on row ${this.#rowIndex}`);
		}
		const rowIndex = this.#rowIndex;
		let colIndex = 0;
		const row = Array.from(cells, (given) => {
			while (this.#claimed.has(`${rowIndex},${colIndex}`)) colIndex++;
			const cell = this.#resolve(given, rowIndex, colIndex);
			for (let i = 0; i < cell.rowSpan; i++) {
				for (let j = 0; j < cell.colSpan; j++) {
					this.#claimed.add(`${rowIndex + i},${colIndex + j}`);
				}
			}
			colIndex += cell.colSpan;
			return cell;
		});
		if (rowIndex === 0) this.#ensure(row);
		const placed = this.#measure(row, rowIndex);
		const tr =
			this.#structure === undefined ? undefined : this.#host.struct("TR");
		if (tr !== undefined) this.#structure?.add(tr);
		for (const cell of placed) {
			const element =
				tr === undefined
					? undefined
					: this.#host.struct(cell.type, {
							scope: cell.scope,
							rowSpan: cell.rowSpan > 1 ? cell.rowSpan : undefined,
							colSpan: cell.colSpan > 1 ? cell.colSpan : undefined,
						});
			if (element !== undefined) tr?.add(element);
			this.#render(cell, element);
		}
		tr?.end();
		if (rowIndex < (this.#options.headerRows ?? 0)) {
			if (rowIndex === 0) this.#headerTop = this.#rowY[0] ?? this.#position.y;
			this.#header.push(...placed);
			this.#headerHeight =
				(this.#rowY[rowIndex] ?? 0) +
				(this.#rowHeights[rowIndex] ?? 0) -
				this.#headerTop;
		}
		this.#host.x = this.#position.x;
		this.#host.y =
			(this.#rowY[rowIndex] ?? 0) + (this.#rowHeights[rowIndex] ?? 0);
		this.#rowIndex++;
		if (lastRow) this.end();
		return this;
	}

	/** Draw what row spans still hold back; the document answers. */
	end(): Document {
		while (this.#buffer.size > 0) this.row([]);
		this.#structure?.end();
		this.#ended = true;
		return this.#document;
	}

	#ensure(row: ResolvedCell[]): void {
		const columns = row.reduce((sum, cell) => sum + cell.colSpan, 0);
		const stars: Array<ResolvedColumn | undefined> = [];
		let starMinimum = 0;
		let unclaimed = this.#maxWidth;
		this.#columnWidths = [];
		for (let index = 0; index < columns; index++) {
			const column = this.#columnStyle(index);
			if (column.width === "*") {
				stars[index] = column;
				starMinimum += column.minWidth;
			} else {
				unclaimed -= column.width;
				this.#columnWidths[index] = column.width;
			}
		}
		let starCount = stars.filter((column) => column !== undefined).length;
		if (starMinimum >= unclaimed) {
			stars.forEach((column, index) => {
				if (column !== undefined) this.#columnWidths[index] = column.minWidth;
			});
		} else {
			stars.forEach((column, index) => {
				if (column === undefined) return;
				let width = Math.max(unclaimed / starCount, column.minWidth);
				if (column.maxWidth > 0) width = Math.min(width, column.maxWidth);
				this.#columnWidths[index] = width;
				unclaimed -= width;
				starCount--;
			});
		}
		let x = this.#position.x;
		this.#columnX = this.#columnWidths.map((width) => {
			const at = x;
			x += width ?? 0;
			return at;
		});
		this.#rowHeights = [];
		this.#rowY = [this.#position.y];
		this.#buffer = new Set();
	}

	#measure(row: ResolvedCell[], rowIndex: number): PlacedCell[] {
		for (const cell of row) this.#buffer.add(cell);
		if (rowIndex > 0) {
			this.#rowY[rowIndex] =
				(this.#rowY[rowIndex - 1] ?? 0) + (this.#rowHeights[rowIndex - 1] ?? 0);
		}
		const style = this.#rowStyle(rowIndex);
		const due: ResolvedCell[] = [];
		for (const cell of this.#buffer) {
			if (cell.rowIndex + cell.rowSpan - 1 === rowIndex) {
				due.push(cell);
				this.#buffer.delete(cell);
			}
		}
		let height = style.height;
		if (height === "auto") {
			height = due
				.map((cell) => this.#place(cell, "auto"))
				.reduce((tallest, cell) => {
					let needed =
						cell.textBounds.height + cell.padding.top + cell.padding.bottom;
					for (let i = 0; i < cell.rowSpan - 1; i++) {
						needed -= this.#rowHeights[cell.rowIndex + i] ?? 0;
					}
					return Math.max(tallest, needed);
				}, 0);
		}
		height = Math.max(height, style.minHeight);
		if (style.maxHeight > 0) height = Math.min(height, style.maxHeight);
		this.#rowHeights[rowIndex] = height;

		const page = this.#host.page;
		const top = this.#rowY[rowIndex] ?? 0;
		if (height > page.contentHeight) {
			process.emitWarning(
				`Row ${rowIndex} needs more than the page's content height and has been clamped`,
				{ code: "VELLUM_TABLE_ROW_CLAMPED" },
			);
			this.#rowHeights[rowIndex] = page.maxY() - top;
		} else if (top + height >= page.maxY()) {
			this.#host.continueOnNewPage();
			let start = this.#host.page.margins.top;
			if (
				this.#header.length > 0 &&
				rowIndex >= (this.#options.headerRows ?? 0)
			) {
				const shift = start - this.#headerTop;
				// Drawn again, the header is pagination: an artifact when tagged.
				const tagged = this.#structure !== undefined;
				if (tagged) {
					this.#host.markContent("Artifact", { type: "Pagination" });
				}
				for (const cell of this.#header) {
					this.#render(
						{ ...cell, y: cell.y + shift, textY: cell.textY + shift },
						undefined,
					);
				}
				if (tagged) this.#host.endMarkedContent();
				start += this.#headerHeight;
			}
			this.#rowY[rowIndex] = start;
		}
		return due.map((cell) =>
			this.#place(cell, this.#rowHeights[rowIndex] ?? 0),
		);
	}

	#place(cell: ResolvedCell, rowHeight: number | "auto"): PlacedCell {
		const host = this.#host;
		let width = 0;
		for (let i = 0; i < cell.colSpan; i++)
			width += this.#columnWidths[cell.colIndex + i] ?? 0;
		let height: number;
		if (rowHeight === "auto") {
			height = host.page.contentHeight;
		} else {
			height = rowHeight;
			for (let i = 0; i < cell.rowSpan - 1; i++)
				height += this.#rowHeights[cell.rowIndex + i] ?? 0;
		}
		const allocatedWidth = width - cell.padding.left - cell.padding.right;
		const allocatedHeight = height - cell.padding.top - cell.padding.bottom;
		const rotation = cell.textOptions.rotation ?? 0;
		const box = rotatedBounds(rotation, allocatedWidth, allocatedHeight);
		const options: TextOptions = {
			align: cell.align.x,
			ellipsis: true,
			stroke: cell.textStroke > 0,
			fill: true,
			width: box.width,
			height: box.height,
			rotation,
			...cell.textOptions,
		};
		let textBounds = { x: 0, y: 0, width: 0, height: 0 };
		if (cell.text) {
			const saved = host.fontState();
			if (cell.customFont) host.useFont(cell.font);
			const flat = host.boundsOfString(cell.text, 0, 0, {
				...options,
				rotation: 0,
			});
			options.width = flat.width;
			options.height = flat.height;
			textBounds = host.boundsOfString(cell.text, 0, 0, options);
			if (cell.customFont) host.useFont(saved);
		}
		const x = this.#columnX[cell.colIndex] ?? this.#position.x;
		const y = this.#rowY[cell.rowIndex] ?? this.#position.y;
		return {
			...cell,
			options,
			x,
			y,
			textX: x + cell.padding.left,
			textY: y + cell.padding.top,
			width,
			height,
			textAllocatedWidth: allocatedWidth,
			textAllocatedHeight: allocatedHeight,
			textBounds,
		};
	}

	/**
	 * `element`: the cell's structure element when the table is tagged; its
	 * background and borders are then decoration, marked as artifacts.
	 */
	#render(cell: PlacedCell, element?: PdfStructureElement): void {
		const host = this.#host;
		if (element !== undefined) host.markContent("Artifact", { type: "Layout" });
		if (cell.backgroundColor !== undefined) {
			host.save();
			host.fillColor(cell.backgroundColor);
			host.rect(cell.x, cell.y, cell.width, cell.height);
			host.fill();
			host.restore();
		}
		this.#border(cell);
		if (cell.debug) {
			host.save();
			host.dash(1, { space: 1 });
			host.lineWidth(1);
			host.strokeOpacity(0.3);
			host.strokeColor("green");
			host.rect(cell.x, cell.y, cell.width, cell.height);
			host.stroke();
			host.restore();
		}
		if (element !== undefined) host.endMarkedContent();
		if (cell.text) {
			if (element !== undefined) {
				element.add(host.markStructureContent(cell.type));
			}
			this.#text(cell);
			if (element !== undefined) host.endMarkedContent();
		}
	}

	#text(cell: PlacedCell): void {
		const host = this.#host;
		const saved = host.fontState();
		if (cell.customFont) host.useFont(cell.font);
		const horizontalShare =
			cell.align.x === "right" ? 1 : cell.align.x === "center" ? 0.5 : 0;
		const verticalShare =
			cell.align.y === "bottom" ? 1 : cell.align.y === "center" ? 0.5 : 0;
		const dx =
			(cell.textAllocatedWidth - cell.textBounds.width) * horizontalShare -
			cell.textBounds.x;
		const dy =
			(cell.textAllocatedHeight - cell.textBounds.height) * verticalShare -
			cell.textBounds.y;
		host.save();
		host.rect(
			cell.textX,
			cell.textY,
			cell.textAllocatedWidth,
			cell.textAllocatedHeight,
		);
		host.clip();
		host.fillColor(cell.textColor);
		host.strokeColor(cell.textStrokeColor);
		if (cell.textStroke > 0) host.lineWidth(cell.textStroke);
		host.text(cell.text, cell.textX + dx, cell.textY + dy, cell.options);
		host.restore();
		if (cell.customFont) host.useFont(saved);
	}

	/**
	 * pdfkit strokes one rectangle in the top colour whenever the four widths
	 * agree, so per-side colours are lost; here they are kept.
	 */
	#border(cell: PlacedCell): void {
		const host = this.#host;
		const { border, borderColor, x, y, width, height } = cell;
		const same =
			border.right === border.top &&
			border.bottom === border.top &&
			border.left === border.top;
		const sameColour =
			JSON.stringify(borderColor.right) === JSON.stringify(borderColor.top) &&
			JSON.stringify(borderColor.bottom) === JSON.stringify(borderColor.top) &&
			JSON.stringify(borderColor.left) === JSON.stringify(borderColor.top);
		if (same && (sameColour || border.top === 0)) {
			if (border.top > 0) {
				host.save();
				host.lineWidth(border.top);
				host.strokeColor(borderColor.top);
				host.rect(x, y, width, height);
				host.stroke();
				host.restore();
			}
			return;
		}
		const sides: Array<
			[number, ColourValue, [number, number], [number, number]]
		> = [
			[border.top, borderColor.top, [x, y], [x + width, y]],
			[
				border.right,
				borderColor.right,
				[x + width, y],
				[x + width, y + height],
			],
			[
				border.bottom,
				borderColor.bottom,
				[x + width, y + height],
				[x, y + height],
			],
			[border.left, borderColor.left, [x, y + height], [x, y]],
		];
		for (const [lineWidth, colour, [x1, y1], [x2, y2]] of sides) {
			if (lineWidth <= 0) continue;
			host.save();
			host.lineWidth(lineWidth);
			host.moveTo(x1, y1);
			host.strokeColor(colour);
			host.lineTo(x2, y2);
			host.stroke();
			host.restore();
		}
	}
}
