/**
 * Authoring a PDF from nothing, with pdfkit's API.
 *
 * ```ts
 * import { PdfDocument } from '@c9up/vellum'
 *
 * const doc = new PdfDocument({ size: 'A4' })
 * doc.pipe(createWriteStream('quote.pdf'))
 * doc.font('Helvetica-Bold').fontSize(18).text('Quote 2026-041')
 * doc.moveDown().font('Helvetica').fontSize(11).text(terms, { align: 'justify' })
 * doc.end()
 * ```
 *
 * The shape is pdfkit 0.20's: the same methods and options, a cursor
 * (`x`, `y`) that drawing moves, `pageAdded`, `bufferPages` with
 * `switchToPage`, a readable stream of the bytes. Layout and writing happen
 * in Rust. Where vellum differs, the difference is named where it happens.
 */

import { Readable } from "node:stream";
import { VellumError } from "../errors.js";
import {
	compose,
	createDocumentNative,
	type NativeDocument,
	standardFontFileNative,
	type TextOptionsInput,
} from "../native.js";
import {
	type ColourValue,
	type PlainColour,
	paintOf,
	plainColour,
} from "./colours.js";
import { type FileSource, readSource, standardFontBytes } from "./files.js";
import {
	normalizeSides,
	pageDimensions,
	type SideDefinition,
	type Sides,
	type Size,
	sizeToPoint,
} from "./sizes.js";
import {
	arc,
	ellipse,
	PathRecorder,
	polygon,
	rect,
	roundedRect,
	svgPath,
} from "./vector.js";

/** A font: a standard name, a name given to {@link PdfDocument.registerFont}, a path, or the file's bytes. */
export type FontSource = FileSource;

export interface PageOptions {
	/** A name from `PAGE_SIZES` (any case) or `[width, height]` in points. Default `letter`. */
	size?: string | readonly [number, number];
	layout?: "portrait" | "landscape";
	margin?: SideDefinition<Size>;
	margins?: SideDefinition<Size>;
	userUnit?: number;
	font?: FontSource;
	fontFamily?: string;
	fontSize?: Size;
}

export interface DocumentInfo {
	Title?: string;
	Author?: string;
	Subject?: string;
	Keywords?: string;
	Creator?: string;
	Producer?: string;
	CreationDate?: Date;
	ModDate?: Date;
	[key: string]: string | Date | undefined;
}

export type PageLayout =
	| "singlePage"
	| "oneColumn"
	| "twoColumnLeft"
	| "twoColumnRight"
	| "twoPageLeft"
	| "twoPageRight";

export interface PdfDocumentOptions
	extends Omit<PageOptions, "fontFamily" | "fontSize"> {
	/**
	 * Default `1.7`. pdfkit's `1.3` (its default) and `1.7ext3` are not
	 * written: the engine writes 1.4 to 2.0.
	 */
	pdfVersion?: "1.4" | "1.5" | "1.6" | "1.7" | "2.0";
	compress?: boolean;
	autoFirstPage?: boolean;
	bufferPages?: boolean;
	info?: DocumentInfo;
	lang?: string;
	displayTitle?: boolean;
	pageLayout?: PageLayout;
	/** Draw JPEGs as stored, not turned by their EXIF orientation. */
	ignoreOrientation?: boolean;
}

export type TextAlign = "left" | "center" | "right" | "justify";

export type TextBaseline =
	| number
	| "svg-middle"
	| "middle"
	| "svg-central"
	| "bottom"
	| "ideographic"
	| "alphabetic"
	| "mathematical"
	| "hanging"
	| "top";

export interface TextOptions {
	/** Wrap width. Default: to the right margin. */
	width?: number;
	/** Where the text stops: no page break past it. */
	height?: number;
	/** `false` draws each line as given, moving `x` along. */
	lineBreak?: boolean;
	/**
	 * Default: left for a left-to-right paragraph, right for a right-to-left
	 * one. pdfkit has no direction, so its default is always left.
	 */
	align?: TextAlign;
	lineGap?: number;
	paragraphGap?: number;
	indent?: number;
	indentAllLines?: boolean;
	columns?: number;
	columnGap?: number;
	ellipsis?: boolean | string;
	continued?: boolean;
	characterSpacing?: number;
	wordSpacing?: number;
	horizontalScaling?: number;
	features?: readonly string[];
	baseline?: TextBaseline;
	/** Degrees, counter-clockwise on the page. */
	rotation?: number;
	/** A fake italic: `true` slants by 0.25, a number is the angle in degrees. */
	oblique?: boolean | number;
	/** With `stroke`, fill as well: pdfkit's render modes. */
	fill?: boolean;
	stroke?: boolean;
	underline?: boolean;
	strike?: boolean;
	/** A URL (http, https, mailto, tel), or a page of this document from zero. */
	link?: string | number;
	/** A named destination to link to. */
	goTo?: string;
	/** Name where each line starts as a destination. */
	destination?: string;
}

export interface ListOptions extends TextOptions {
	listType?: "bullet" | "numbered" | "lettered";
	bulletRadius?: number;
	/** From the bullet to the text. */
	textIndent?: number;
	/** What each nested level adds. */
	bulletIndent?: number;
}

/** A list's items: nested arrays are sub-lists. */
export type ListItems = ReadonlyArray<string | ListItems>;

export interface WidthOptions {
	characterSpacing?: number;
	horizontalScaling?: number;
	features?: readonly string[];
}

type Matrix = [number, number, number, number, number, number];

/**
 * A gradient, built with `stop()` and painted with `fill(gradient)` or
 * `fillColor(gradient)`. Its coordinates are in the user space it is
 * painted in.
 */
export abstract class PdfGradient {
	readonly #stops: { offset: number; colour: PlainColour; opacity: number }[] =
		[];
	#transform: Matrix = [1, 0, 0, 1, 0, 0];
	#handle: number | undefined;
	readonly #register: (gradient: PdfGradient) => number;

	protected constructor(register: (gradient: PdfGradient) => number) {
		this.#register = register;
	}

	/** `offset` from 0 to 1; every stop in one colour space. */
	stop(offset: number, colour: ColourValue, opacity = 1): this {
		this.#stops.push({ offset, colour: plainColour(colour), opacity });
		this.#handle = undefined;
		return this;
	}

	setTransform(
		m11: number,
		m12: number,
		m21: number,
		m22: number,
		dx: number,
		dy: number,
	): this {
		this.#transform = [m11, m12, m21, m22, dx, dy];
		this.#handle = undefined;
		return this;
	}

	abstract readonly kind: "linear" | "radial";
	abstract readonly coords: readonly number[];

	/** The engine's handle, registered again only once the gradient changed. */
	handle(): number {
		if (this.#handle === undefined) this.#handle = this.#register(this);
		return this.#handle;
	}

	get stops(): ReadonlyArray<{
		offset: number;
		colour: PlainColour;
		opacity: number;
	}> {
		return this.#stops;
	}

	get transform(): readonly number[] {
		return this.#transform;
	}
}

export class PdfLinearGradient extends PdfGradient {
	readonly kind = "linear";
	readonly coords: readonly number[];

	constructor(
		register: (gradient: PdfGradient) => number,
		x1: number,
		y1: number,
		x2: number,
		y2: number,
	) {
		super(register);
		this.coords = [x1, y1, x2, y2];
	}
}

export class PdfRadialGradient extends PdfGradient {
	readonly kind = "radial";
	readonly coords: readonly number[];

	constructor(
		register: (gradient: PdfGradient) => number,
		x1: number,
		y1: number,
		r1: number,
		x2: number,
		y2: number,
		r2: number,
	) {
		super(register);
		this.coords = [x1, y1, r1, x2, y2, r2];
	}
}

/** An image opened with {@link PdfDocument.openImage}: its size in pixels. */
export interface PdfImage {
	readonly width: number;
	readonly height: number;
	/** EXIF orientation of a JPEG, 1 to 8. */
	readonly orientation: number;
}

/** An image: a path, a `data:` URL, its bytes, or one already opened. */
export type ImageSource = FileSource | PdfImage;

export interface ImageOptions {
	x?: number;
	y?: number;
	width?: number;
	height?: number;
	scale?: number;
	/** Fit inside `[width, height]`, keeping the proportions. */
	fit?: readonly [number, number];
	/**
	 * Cover `[width, height]`, keeping the proportions. What overflows is
	 * cut off; pdfkit draws it past the box.
	 */
	cover?: readonly [number, number];
	align?: "left" | "center" | "right";
	valign?: "top" | "center" | "bottom";
	link?: string | number;
	goTo?: string;
	destination?: string;
	opacity?: number;
	ignoreOrientation?: boolean;
}

/** A colour, or a gradient. */
export type Colour = ColourValue | PdfGradient;

export type FillRule = "even-odd" | "evenodd" | "non-zero" | "nonzero";

const FILL_RULE = /^(even-?odd|non-?zero)$/;

function isFillRule(value: unknown): value is FillRule {
	return typeof value === "string" && FILL_RULE.test(value);
}

/** A page of the document: its size and margins, in points. */
export class PdfPage {
	readonly width: number;
	readonly height: number;
	readonly size: string | readonly [number, number];
	readonly layout: "portrait" | "landscape";
	/**
	 * Settable, as in pdfkit: a footer is commonly written by setting
	 * `margins.bottom` to 0 for a moment.
	 */
	readonly margins: Sides<number>;
	readonly userUnit: number;
	/** The options the page was added with, as `continueOnNewPage` reuses them. */
	readonly options: PageOptions;

	constructor(
		dimensions: readonly [number, number],
		margins: Sides<number>,
		options: PageOptions,
		sync: (margins: Sides<number>) => void,
	) {
		[this.width, this.height] = dimensions;
		this.size = options.size ?? "letter";
		this.layout = options.layout ?? "portrait";
		const values = { ...margins };
		const live: Sides<number> = { ...margins };
		for (const side of SIDES) {
			Object.defineProperty(live, side, {
				enumerable: true,
				get: () => values[side],
				set: (value: number) => {
					sync({ ...values, [side]: value });
					values[side] = value;
				},
			});
		}
		this.margins = live;
		this.userUnit = options.userUnit ?? 1;
		this.options = options;
	}

	get contentWidth(): number {
		return this.width - this.margins.left - this.margins.right;
	}

	get contentHeight(): number {
		return this.height - this.margins.top - this.margins.bottom;
	}

	maxY(): number {
		return this.height - this.margins.bottom;
	}
}

const PAGE_KEYS = ["size", "layout", "userUnit"] as const;
const SIDES = ["top", "right", "bottom", "left"] as const;

interface ResolvedText {
	options: TextOptions;
	native: TextOptionsInput;
}

function infoInput(info: DocumentInfo) {
	const custom: string[][] = [];
	const known = new Set([
		"Title",
		"Author",
		"Subject",
		"Keywords",
		"Creator",
		"Producer",
		"CreationDate",
		"ModDate",
	]);
	for (const [key, value] of Object.entries(info)) {
		if (known.has(key) || value === undefined) continue;
		custom.push([key, value instanceof Date ? value.toISOString() : value]);
	}
	return {
		title: info.Title,
		author: info.Author,
		subject: info.Subject,
		keywords: info.Keywords,
		creator: info.Creator,
		producer: info.Producer,
		creationDate: (info.CreationDate ?? new Date()).getTime(),
		modDate: info.ModDate?.getTime(),
		custom,
	};
}

export class PdfDocument extends Readable {
	readonly options: PdfDocumentOptions;
	/** The document information, written when the document ends. */
	readonly info: DocumentInfo;
	readonly #native: NativeDocument;
	readonly #pages: PdfPage[] = [];
	readonly #fonts = new Map<string, number>();
	readonly #registered = new Map<
		string,
		{ src: FontSource; family: string | undefined }
	>();
	readonly #remSize: number;
	#textOptions: TextOptions | undefined;
	readonly #path = new PathRecorder();
	readonly #spots = new Map<string, number>();
	readonly #images = new Map<string, PdfImage>();
	readonly #imageHandles = new WeakMap<PdfImage, number>();
	#ended = false;

	constructor(options: PdfDocumentOptions = {}) {
		super();
		this.options = options;
		this.info = { CreationDate: new Date(), ...options.info };
		this.#native = createDocumentNative({
			lang: options.lang,
			displayTitle: options.displayTitle,
			pageLayout: options.pageLayout,
			pdfVersion: options.pdfVersion,
			compress: options.compress,
			bufferPages: options.bufferPages,
		});
		this.#remSize = 12;
		this.font(options.font ?? "Helvetica");
		if (options.autoFirstPage !== false) this.addPage();
	}

	override _read(): void {
		// The bytes are pushed whole by end().
	}

	get x(): number {
		return this.#native.getX();
	}

	set x(value: number) {
		compose(() => this.#native.setX(value));
	}

	get y(): number {
		return this.#native.getY();
	}

	set y(value: number) {
		compose(() => this.#native.setY(value));
	}

	/** The page drawing goes to. */
	get page(): PdfPage {
		const index = this.#native.currentPage();
		const page = index === null ? undefined : this.#pages[index];
		if (page === undefined) {
			throw new VellumError(
				"NO_PAGE",
				"there is no current page: call addPage() first",
			);
		}
		return page;
	}

	#sizeContext(page: { width: number; height: number }, percentageOf?: number) {
		return {
			fontSize: this.#native.getFontSize(),
			remSize: this.#remSize,
			lineHeight: () => this.currentLineHeight(),
			zeroWidth: () => this.widthOfString("0"),
			page,
			percentageOf,
		};
	}

	/**
	 * pdfkit's `sizeToPoint`. A string that is not a length is refused,
	 * where pdfkit reads it as 0.
	 */
	sizeToPoint(
		size: Size | boolean | undefined,
		fallback: Size | boolean = 0,
		page: { width: number; height: number } = this.page,
		percentageOf?: number,
	): number {
		return sizeToPoint(size, fallback, this.#sizeContext(page, percentageOf));
	}

	/**
	 * Start a new page. Options given are merged over the document's page
	 * options — pdfkit replaces them instead, so its `addPage({ margin: 10 })`
	 * falls back to a letter page.
	 */
	addPage(options?: PageOptions): this {
		this.#open();
		const merged = this.#pageOptions(options);
		if (options?.font !== undefined)
			this.font(options.font, options.fontFamily);
		if (options?.fontSize !== undefined) this.fontSize(options.fontSize);
		const dimensions = pageDimensions(merged.size, merged.layout);
		const page = { width: dimensions[0], height: dimensions[1] };
		const margins = normalizeSides(
			merged.margin ?? merged.margins,
			72,
			(value) => this.sizeToPoint(value, 0, page),
		);
		compose(() =>
			this.#native.addPage({
				width: dimensions[0],
				height: dimensions[1],
				...margins,
				userUnit: merged.userUnit,
			}),
		);
		const index = this.#pages.length;
		this.#pages.push(
			new PdfPage(dimensions, margins, merged, (next) =>
				compose(() =>
					this.#native.setMargins(
						index,
						next.top,
						next.right,
						next.bottom,
						next.left,
					),
				),
			),
		);
		this.emit("pageAdded");
		return this;
	}

	#pageOptions(options: PageOptions | undefined): PageOptions {
		const merged: PageOptions = {};
		for (const key of PAGE_KEYS) {
			const value = options?.[key] ?? this.options[key];
			if (value !== undefined) Object.assign(merged, { [key]: value });
		}
		const ownMargin =
			options?.margin !== undefined || options?.margins !== undefined;
		const margin = ownMargin ? options?.margin : this.options.margin;
		const margins = ownMargin ? options?.margins : this.options.margins;
		if (margin !== undefined) merged.margin = margin;
		if (margins !== undefined) merged.margins = margins;
		return merged;
	}

	/** A new page like the current one: what text does when it runs out of page. */
	continueOnNewPage(options?: PageOptions): this {
		return this.addPage(options ?? this.page.options);
	}

	bufferedPageRange(): { start: number; count: number } {
		return this.#native.bufferedPageRange();
	}

	switchToPage(index: number): PdfPage {
		compose(() => this.#native.switchToPage(index));
		return this.page;
	}

	/** Close the pages written so far: they can no longer be switched to. */
	flushPages(): void {
		compose(() => this.#native.flushPages());
	}

	registerFont(name: string, src: FontSource, family?: string): this {
		this.#registered.set(name, { src, family });
		return this;
	}

	font(src: FontSource, family?: string | number, size?: Size): this {
		this.#open();
		let face = typeof family === "number" ? undefined : family;
		const fontSize = typeof family === "number" ? family : size;
		let source = src;
		let cacheKey: string | undefined;
		const registered =
			typeof src === "string" ? this.#registered.get(src) : undefined;
		if (typeof src === "string" && registered !== undefined) {
			cacheKey = src;
			source = registered.src;
			face = registered.family;
		} else {
			const named = face ?? src;
			cacheKey = typeof named === "string" ? named : undefined;
		}
		if (fontSize !== undefined) this.fontSize(fontSize);

		const known =
			cacheKey === undefined ? undefined : this.#fonts.get(cacheKey);
		if (known !== undefined) {
			compose(() => this.#native.setFont(known));
			return this;
		}
		const standard =
			typeof source === "string" ? standardFontFileNative(source) : null;
		const bytes =
			standard !== null ? standardFontBytes(standard) : readSource(source);
		const handle = compose(() =>
			this.#native.loadFont(
				bytes,
				face,
				standard !== null && typeof source === "string" ? source : undefined,
			),
		);
		if (cacheKey !== undefined) this.#fonts.set(cacheKey, handle);
		compose(() => this.#native.setFont(handle));
		return this;
	}

	fontSize(size: Size): this {
		const points = sizeToPoint(
			size,
			0,
			this.#sizeContext(
				this.#pages.length > 0 ? this.page : { width: 0, height: 0 },
			),
		);
		compose(() => this.#native.setFontSize(points));
		return this;
	}

	currentLineHeight(includeGap?: boolean): number {
		return compose(() => this.#native.currentLineHeight(includeGap));
	}

	lineGap(gap: number): this {
		compose(() => this.#native.setLineGap(gap));
		return this;
	}

	moveDown(lines = 1): this {
		compose(() => this.#native.moveDown(lines));
		return this;
	}

	moveUp(lines = 1): this {
		compose(() => this.#native.moveUp(lines));
		return this;
	}

	widthOfString(text: string, options: WidthOptions = {}): number {
		return compose(() =>
			this.#native.widthOfString(
				text,
				options.characterSpacing,
				options.horizontalScaling,
				options.features === undefined ? undefined : [...options.features],
			),
		);
	}

	text(text: unknown, options?: TextOptions): this;
	text(text: unknown, x?: number, y?: number, options?: TextOptions): this;
	text(
		text: unknown,
		x?: number | TextOptions,
		y?: number,
		options?: TextOptions,
	): this {
		this.#open();
		const resolved = this.#initOptions(x, y, options);
		this.#textOptions = resolved.options.continued
			? resolved.options
			: undefined;
		const value = text === null || text === undefined ? "" : String(text);
		let step = compose(() => this.#native.text(value, resolved.native));
		while (step === "needPage") {
			this.continueOnNewPage();
			step = compose(() => this.#native.resumeText());
		}
		return this;
	}

	/**
	 * pdfkit's `list`: bullets, numbers (`1.`) or letters (`A.` … `Z.`,
	 * `AA.`), nested arrays as sub-lists numbered from 1.
	 */
	list(
		items: ListItems,
		x?: number | ListOptions,
		y?: number,
		options?: ListOptions,
	): this {
		this.#open();
		const given: ListOptions = typeof x === "object" ? x : (options ?? {});
		const listType = given.listType ?? "bullet";
		const unit = Math.round(
			(compose(() => this.#native.ascender()) / 1000) *
				this.#native.getFontSize(),
		);
		const radius = given.bulletRadius || unit / 3;
		const indent =
			given.textIndent || (listType === "bullet" ? radius * 5 : unit * 2);
		const itemIndent =
			given.bulletIndent || (listType === "bullet" ? radius * 8 : unit * 2);
		const flat: Array<{ text: string; level: number; number: number }> = [];
		const flatten = (list: ListItems, level: number) => {
			let number = 1;
			for (const item of list) {
				if (typeof item === "string") {
					flat.push({ text: item, level, number: number++ });
				} else {
					flatten(item, level + 1);
				}
			}
		};
		flatten(items, 1);
		const resolved = this.#initOptions(
			typeof x === "object" ? given : x,
			y,
			given,
		);
		for (const { text, level, number } of flat) {
			const label =
				listType === "numbered"
					? `${number}.`
					: listType === "lettered"
						? `${String.fromCharCode(((number - 1) % 26) + 65).repeat(Math.floor((number - 1) / 26) + 1)}.`
						: undefined;
			const item =
				label === undefined
					? { level, radius, midLine: unit / 2, indent, itemIndent }
					: { level, label, indent, itemIndent };
			let step = compose(() =>
				this.#native.listItem(text, resolved.native, item),
			);
			while (step === "needPage") {
				this.continueOnNewPage();
				step = compose(() => this.#native.resumeText());
			}
		}
		return this;
	}

	heightOfString(text: unknown, options?: TextOptions): number {
		const resolved = this.#initOptions(options, undefined, undefined);
		const value = text === null || text === undefined ? "" : String(text);
		return compose(() => this.#native.heightOfString(value, resolved.native));
	}

	/**
	 * The box of what `text()` would draw from (`x`, `y`), rotation
	 * included. The cursor moves to (`x`, `y`) when they are given.
	 */
	boundsOfString(
		text: unknown,
		x?: number | TextOptions,
		y?: number,
		options?: TextOptions,
	): { x: number; y: number; width: number; height: number } {
		const resolved = this.#initOptions(x, y, options);
		const value = text === null || text === undefined ? "" : String(text);
		const { x: left, y: top } = this;
		const { width, height } = compose(() =>
			this.#native.boundsOfString(value, resolved.native),
		);
		const rotation = normalizeRotation(resolved.options.rotation);
		switch (rotation) {
			case 0:
				return { x: left, y: top, width, height };
			case 90:
				return { x: left, y: top - width, width: height, height: width };
			case 180:
				return { x: left - width, y: top - height, width, height };
			case 270:
				return { x: left - height, y: top, width: height, height: width };
		}
		const cos = Math.cos((rotation * Math.PI) / 180);
		const sin = Math.sin((rotation * Math.PI) / 180);
		const xs = [
			left,
			left + width * cos,
			left + width * cos + height * sin,
			left + height * sin,
		];
		const ys = [
			top,
			top - width * sin,
			top - width * sin + height * cos,
			top + height * cos,
		];
		const x0 = Math.min(...xs);
		const y0 = Math.min(...ys);
		return {
			x: x0,
			y: y0,
			width: Math.max(...xs) - x0,
			height: Math.max(...ys) - y0,
		};
	}

	/** pdfkit's `_initOptions`: merge a continued text's options, place the cursor, default the width. */
	#initOptions(
		x: number | TextOptions | undefined,
		y: number | undefined,
		given: TextOptions | undefined,
	): ResolvedText {
		const options: TextOptions = {
			...(typeof x === "object" ? x : given),
		};
		if (this.#textOptions !== undefined) {
			for (const [key, value] of Object.entries(this.#textOptions)) {
				if (key !== "continued" && Reflect.get(options, key) === undefined) {
					Reflect.set(options, key, value);
				}
			}
		}
		if (typeof x === "number") this.x = x;
		if (y !== undefined) this.y = y;
		if (options.lineBreak !== false) {
			if (options.width === undefined) {
				const page = this.page;
				options.width = page.width - this.x - page.margins.right;
			}
			options.width = Math.max(options.width, 0);
		}
		const baseline = options.baseline;
		return {
			options,
			native: {
				width: options.lineBreak === false ? undefined : options.width,
				height: options.height,
				align: options.align,
				lineGap: options.lineGap,
				paragraphGap: options.paragraphGap,
				indent: options.indent,
				indentAllLines: options.indentAllLines,
				columns:
					options.columns === undefined || options.columns < 1
						? 1
						: options.columns,
				columnGap: options.columnGap ?? 18,
				ellipsis:
					options.ellipsis === true
						? "…"
						: typeof options.ellipsis === "string"
							? options.ellipsis
							: undefined,
				continued: options.continued,
				characterSpacing: options.characterSpacing,
				wordSpacing: options.wordSpacing,
				horizontalScaling: options.horizontalScaling || 100,
				features:
					options.features === undefined ? undefined : [...options.features],
				baseline: typeof baseline === "string" ? baseline : undefined,
				baselineOffset: typeof baseline === "number" ? baseline : undefined,
				rotation: normalizeRotation(options.rotation),
				oblique:
					options.oblique === true
						? -0.25
						: typeof options.oblique === "number"
							? -Math.tan((options.oblique * Math.PI) / 180)
							: undefined,
				fill: options.fill,
				stroke: options.stroke,
				underline: options.underline,
				strike: options.strike,
				link: typeof options.link === "string" ? options.link : undefined,
				linkPage: typeof options.link === "number" ? options.link : undefined,
				goTo: options.goTo,
				destination: options.destination,
			},
		};
	}

	save(): this {
		compose(() => this.#native.save());
		return this;
	}

	/** pdfkit writes an unmatched restore into the page; here it is refused. */
	restore(): this {
		compose(() => this.#native.restore());
		return this;
	}

	transform(
		m11: number,
		m12: number,
		m21: number,
		m22: number,
		dx: number,
		dy: number,
	): this {
		if (
			m11 === 1 &&
			m12 === 0 &&
			m21 === 0 &&
			m22 === 1 &&
			dx === 0 &&
			dy === 0
		) {
			return this;
		}
		compose(() => this.#native.transform([m11, m12, m21, m22, dx, dy]));
		return this;
	}

	translate(x: number, y: number): this {
		return this.transform(1, 0, 0, 1, x, y);
	}

	/** Degrees, clockwise on the page, about `origin` (default the user-space origin). */
	rotate(
		angle: number,
		options: { origin?: readonly [number, number] } = {},
	): this {
		const radians = (angle * Math.PI) / 180;
		const cos = Math.cos(radians);
		const sin = Math.sin(radians);
		let x = 0;
		let y = 0;
		if (options.origin !== undefined) {
			[x, y] = options.origin;
			const rotatedX = x * cos - y * sin;
			const rotatedY = x * sin + y * cos;
			x -= rotatedX;
			y -= rotatedY;
		}
		return this.transform(cos, sin, -sin, cos, x, y);
	}

	scale(
		xFactor: number,
		yFactor?: number | { origin?: readonly [number, number] },
		options: { origin?: readonly [number, number] } = {},
	): this {
		const factor = typeof yFactor === "number" ? yFactor : xFactor;
		const origin =
			typeof yFactor === "object" ? yFactor.origin : options.origin;
		let x = 0;
		let y = 0;
		if (origin !== undefined) {
			[x, y] = origin;
			x -= xFactor * x;
			y -= factor * y;
		}
		return this.transform(xFactor, 0, 0, factor, x, y);
	}

	lineWidth(width: number): this {
		compose(() => this.#native.setLineWidth(width));
		return this;
	}

	lineCap(cap: "butt" | "round" | "square" | 0 | 1 | 2): this {
		const name =
			typeof cap === "number"
				? (["butt", "round", "square"] as const)[cap]
				: cap.toLowerCase();
		compose(() => this.#native.setLineCap(String(name)));
		return this;
	}

	lineJoin(join: "miter" | "round" | "bevel" | 0 | 1 | 2): this {
		const name =
			typeof join === "number"
				? (["miter", "round", "bevel"] as const)[join]
				: join.toLowerCase();
		compose(() => this.#native.setLineJoin(String(name)));
		return this;
	}

	miterLimit(limit: number): this {
		compose(() => this.#native.setMiterLimit(limit));
		return this;
	}

	dash(
		length: number | readonly number[],
		options: { space?: number; phase?: number } = {},
	): this {
		const lengths =
			typeof length === "number"
				? [length, options.space || length]
				: [...length];
		compose(() => this.#native.setDash(lengths, options.phase || 0));
		return this;
	}

	undash(): this {
		compose(() => this.#native.setDash(undefined, undefined));
		return this;
	}

	moveTo(x: number, y: number): this {
		this.#path.moveTo(x, y);
		return this;
	}

	lineTo(x: number, y: number): this {
		this.#path.lineTo(x, y);
		return this;
	}

	bezierCurveTo(
		cp1x: number,
		cp1y: number,
		cp2x: number,
		cp2y: number,
		x: number,
		y: number,
	): this {
		this.#path.bezierCurveTo(cp1x, cp1y, cp2x, cp2y, x, y);
		return this;
	}

	/**
	 * A true quadratic curve. pdfkit writes PDF's `v` operator instead: a
	 * cubic whose first handle is the current point, which bends differently.
	 */
	quadraticCurveTo(cpx: number, cpy: number, x: number, y: number): this {
		this.#path.quadraticCurveTo(cpx, cpy, x, y);
		return this;
	}

	closePath(): this {
		this.#path.closePath();
		return this;
	}

	rect(x: number, y: number, width: number, height: number): this {
		rect(this.#path, x, y, width, height);
		return this;
	}

	/** One radius, or `[topLeft, topRight, bottomRight, bottomLeft]`. */
	roundedRect(
		x: number,
		y: number,
		width: number,
		height: number,
		radius?: number | readonly number[],
	): this {
		roundedRect(this.#path, x, y, width, height, radius);
		return this;
	}

	ellipse(x: number, y: number, radiusX: number, radiusY?: number): this {
		ellipse(this.#path, x, y, radiusX, radiusY);
		return this;
	}

	circle(x: number, y: number, radius: number): this {
		return this.ellipse(x, y, radius);
	}

	/** Angles in radians. */
	arc(
		x: number,
		y: number,
		radius: number,
		startAngle: number,
		endAngle: number,
		anticlockwise?: boolean,
	): this {
		arc(this.#path, x, y, radius, startAngle, endAngle, anticlockwise);
		return this;
	}

	polygon(...points: ReadonlyArray<readonly [number, number]>): this {
		polygon(this.#path, points);
		return this;
	}

	/** SVG path data (`M10 10 h 50 a 5 5 0 0 1 5 5 Z`). */
	path(data: string): this {
		svgPath(this.#path, data);
		return this;
	}

	#paint(fill: FillRule | undefined | null, stroke: boolean): this {
		const { verbs, coords } = this.#path.take();
		if (verbs.length === 0) return this;
		compose(() =>
			this.#native.paintPath(
				verbs,
				coords,
				fill === null ? undefined : (fill ?? "non-zero"),
				stroke,
			),
		);
		return this;
	}

	fill(colour?: Colour | FillRule, rule?: FillRule): this {
		if (isFillRule(colour)) return this.#paint(colour, false);
		if (colour !== undefined) this.fillColor(colour);
		return this.#paint(rule, false);
	}

	stroke(colour?: Colour): this {
		if (colour !== undefined) this.strokeColor(colour);
		return this.#paint(null, true);
	}

	fillAndStroke(
		fillColour?: Colour | FillRule,
		strokeColour?: Colour | FillRule,
		rule?: FillRule,
	): this {
		let fill = fillColour;
		let stroke = strokeColour ?? fillColour;
		let windingRule = rule;
		if (isFillRule(fill)) {
			windingRule = fill;
			fill = undefined;
		}
		if (isFillRule(stroke)) {
			windingRule = stroke;
			stroke = fill;
		}
		if (fill !== undefined) {
			this.fillColor(fill);
			if (stroke !== undefined) this.strokeColor(stroke);
		}
		return this.#paint(windingRule, true);
	}

	/** Clip to the path until the next `restore()`. */
	clip(rule?: FillRule): this {
		const { verbs, coords } = this.#path.take();
		compose(() => this.#native.clipPath(verbs, coords, rule));
		return this;
	}

	#paintInput(colour: Colour) {
		if (colour instanceof PdfGradient) {
			return { kind: "gradient", handle: colour.handle() };
		}
		return paintOf(colour, this.#spots);
	}

	fillColor(colour: Colour, opacity?: number): this {
		const paint = this.#paintInput(colour);
		compose(() => this.#native.setFillPaint(paint));
		if (opacity !== undefined) this.fillOpacity(opacity);
		return this;
	}

	strokeColor(colour: Colour, opacity?: number): this {
		const paint = this.#paintInput(colour);
		compose(() => this.#native.setStrokePaint(paint));
		if (opacity !== undefined) this.strokeOpacity(opacity);
		return this;
	}

	opacity(opacity: number): this {
		return this.fillOpacity(opacity).strokeOpacity(opacity);
	}

	fillOpacity(opacity: number): this {
		this.#native.setFillOpacity(opacity);
		return this;
	}

	strokeOpacity(opacity: number): this {
		this.#native.setStrokeOpacity(opacity);
		return this;
	}

	#registerGradient = (gradient: PdfGradient): number =>
		compose(() =>
			this.#native.addGradient({
				kind: gradient.kind,
				coords: [...gradient.coords],
				stops: gradient.stops.map((stop) => ({
					offset: stop.offset,
					kind: stop.colour.kind,
					values: stop.colour.values,
					opacity: stop.opacity,
				})),
				transform: [...gradient.transform],
			}),
		);

	linearGradient(
		x1: number,
		y1: number,
		x2: number,
		y2: number,
	): PdfLinearGradient {
		return new PdfLinearGradient(this.#registerGradient, x1, y1, x2, y2);
	}

	radialGradient(
		x1: number,
		y1: number,
		r1: number,
		x2: number,
		y2: number,
		r2: number,
	): PdfRadialGradient {
		return new PdfRadialGradient(
			this.#registerGradient,
			x1,
			y1,
			r1,
			x2,
			y2,
			r2,
		);
	}

	/** A spot colour, then used by its name; CMYK from 0 to 100. */
	addSpotColor(name: string, c: number, m: number, y: number, k: number): this {
		const cmyk = [c, m, y, k].map((part) => (part * 255) / 100);
		const handle = compose(() => this.#native.addSpotColour(name, cmyk));
		this.#spots.set(name, handle);
		return this;
	}

	/**
	 * A link over a rectangle, to a URL or to a page of this document
	 * (from zero). Only http, https, mailto and tel URLs are written: pdfkit
	 * writes any, `javascript:` and `file:` included.
	 */
	link(
		x: number,
		y: number,
		width: number,
		height: number,
		target: string | number,
	): this {
		compose(() =>
			typeof target === "number"
				? this.#native.link(x, y, width, height, undefined, target, undefined)
				: this.#native.link(x, y, width, height, target, undefined, undefined),
		);
		return this;
	}

	/** A link over a rectangle to a named destination. */
	goTo(
		x: number,
		y: number,
		width: number,
		height: number,
		name: string,
	): this {
		compose(() =>
			this.#native.link(x, y, width, height, undefined, undefined, name),
		);
		return this;
	}

	/**
	 * Name a place on the current page, for `goTo`. `XYZ` (the default) at
	 * `left`, `top`; without them, the page's top-left corner.
	 */
	addNamedDestination(
		name: string,
		...position: [fit?: "XYZ", left?: number | null, top?: number | null]
	): this {
		const [, left, top] = position;
		compose(() => this.#native.addNamedDestination(name, left ?? 0, top ?? 0));
		return this;
	}

	/** Open an image once, to draw it several times or read its size. */
	openImage(src: ImageSource): PdfImage {
		if (typeof src === "string") {
			const known = this.#images.get(src);
			if (known !== undefined) return known;
		}
		if (isOpenedImage(src)) {
			if (!this.#imageHandles.has(src)) {
				throw new VellumError(
					"INVALID_IMAGE",
					"this image was opened by another document",
				);
			}
			return src;
		}
		const bytes = readSource(src);
		const info = compose(() => this.#native.loadImage(bytes));
		const image: PdfImage = Object.freeze({
			width: info.width,
			height: info.height,
			orientation: info.orientation,
		});
		this.#imageHandles.set(image, info.handle);
		if (typeof src === "string") this.#images.set(src, image);
		return image;
	}

	/** pdfkit's `image`: sized, fitted or covered, turned by its EXIF orientation. */
	image(
		src: ImageSource,
		x?: number | ImageOptions,
		y?: number,
		options: ImageOptions = {},
	): this {
		this.#open();
		const given = typeof x === "object" ? x : options;
		const ignoreOrientation =
			given.ignoreOrientation ||
			(given.ignoreOrientation !== false &&
				this.options.ignoreOrientation === true);
		const inDocumentFlow = typeof y !== "number";
		let left = (typeof x === "number" ? x : undefined) ?? given.x ?? this.x;
		let top = y ?? given.y ?? this.y;

		const image = this.openImage(src);
		const handle = this.#imageHandles.get(image);
		if (handle === undefined) {
			throw new VellumError(
				"INVALID_IMAGE",
				"this image was not opened by this document",
			);
		}
		let { width, height } = image;
		if (!ignoreOrientation && image.orientation > 4)
			[width, height] = [height, width];

		let w = given.width || width;
		let h = given.height || height;
		let box: readonly [number, number] | undefined;
		if (given.width && !given.height) {
			h = height * (w / width);
		} else if (given.height && !given.width) {
			w = width * (h / height);
		} else if (given.scale) {
			w = width * given.scale;
			h = height * given.scale;
		} else if (given.fit) {
			box = given.fit;
			const [bw, bh] = box;
			if (width / height > bw / bh) {
				w = bw;
				h = bw / (width / height);
			} else {
				h = bh;
				w = bh * (width / height);
			}
		} else if (given.cover) {
			box = given.cover;
			const [bw, bh] = box;
			if (width / height > bw / bh) {
				h = bh;
				w = bh * (width / height);
			} else {
				w = bw;
				h = bw / (width / height);
			}
		}
		const boxLeft = left;
		const boxTop = top;
		if (box !== undefined) {
			const [bw, bh] = box;
			if (given.align === "center") left += bw / 2 - w / 2;
			else if (given.align === "right") left += bw - w;
			if (given.valign === "center") top += bh / 2 - h / 2;
			else if (given.valign === "bottom") top += bh - h;
		}

		if (given.link !== undefined) this.link(left, top, w, h, given.link);
		if (given.goTo !== undefined) this.goTo(left, top, w, h, given.goTo);
		if (given.destination !== undefined) {
			this.addNamedDestination(given.destination, "XYZ", left, top);
		}
		if (inDocumentFlow) this.y += h;

		// Only `cover` chose the box; `fit` keeps the image inside it.
		const clip = given.fit === undefined ? box : undefined;
		if (clip !== undefined) {
			this.save();
			this.rect(boxLeft, boxTop, clip[0], clip[1]).clip();
		}
		compose(() =>
			this.#native.drawImage(
				handle,
				left,
				top,
				w,
				h,
				ignoreOrientation ? 1 : image.orientation,
				given.opacity,
			),
		);
		if (clip !== undefined) this.restore();
		return this;
	}

	#open(): void {
		if (this.#ended) {
			throw new VellumError(
				"DOCUMENT_ENDED",
				"the document has ended; nothing more can be added to it",
			);
		}
	}

	/** Write the document: its bytes are then read from the stream. */
	end(): this {
		this.#open();
		const info = infoInput(this.info);
		compose(() => this.#native.setInfo(info));
		const bytes = compose(() => this.#native.end());
		this.#ended = true;
		this.push(bytes);
		this.push(null);
		return this;
	}
}

function isOpenedImage(source: ImageSource): source is PdfImage {
	return (
		typeof source === "object" &&
		!(source instanceof Uint8Array) &&
		!(source instanceof ArrayBuffer) &&
		!(source instanceof URL)
	);
}

/** Degrees brought into [0, 360), as pdfkit does. */
function normalizeRotation(rotation: number | undefined): number {
	const turned = Number(rotation ?? 0) % 360;
	return turned < 0 ? turned + 360 : turned;
}

/** Every byte of a document, once it has ended. */
export function toBytes(document: PdfDocument): Promise<Uint8Array> {
	return new Promise((resolve, reject) => {
		const chunks: Buffer[] = [];
		const onData = (chunk: Buffer) => chunks.push(chunk);
		const cleanup = () => {
			document.off("data", onData);
			document.off("end", onEnd);
			document.off("error", onError);
		};
		const onEnd = () => {
			cleanup();
			resolve(new Uint8Array(Buffer.concat(chunks)));
		};
		const onError = (error: Error) => {
			cleanup();
			reject(error);
		};
		document.on("data", onData);
		document.on("end", onEnd);
		document.on("error", onError);
	});
}
