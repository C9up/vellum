// GENERATED FROM THE RUST — do not edit.
//
// Produced by scripts/generate-napi-types.mjs from napi-derive's type-def
// output. Editing this file by hand puts it back where it started: a
// description that can disagree with the code it describes.

export interface DocumentInfoInput {
	title?: string;
	author?: string;
	subject?: string;
	keywords?: string;
	creator?: string;
	producer?: string;
	/** Milliseconds since the Unix epoch. */
	creationDate?: number;
	modDate?: number;
	/** Any other key, as `[key, value]` pairs. */
	custom?: Array<Array<string>>;
}

export interface DocumentSettingsInput {
	tagged?: boolean;
	/** `PDF/A-1b`, `PDF/A-2a`, `PDF/UA`… */
	subset?: string;
	lang?: string;
	displayTitle?: boolean;
	pageLayout?: string;
	pdfVersion?: string;
	compress?: boolean;
	bufferPages?: boolean;
	/** The largest file `file()` embeds, in bytes. Default 64 MiB. */
	maxFileBytes?: number;
}

export interface PageSetupInput {
	width: number;
	height: number;
	top: number;
	right: number;
	bottom: number;
	left: number;
	userUnit?: number;
}

/** An item of `list()`: a bullet (`radius`, `midLine`) or a `label`. */

export interface ListItemInput {
	level: number;
	radius?: number;
	midLine?: number;
	label?: string;
	indent: number;
	itemIndent: number;
}

/**
 * What a marked sequence is: `artifact` (with `type`), `span`, or
 * `structure`.
 */

export interface MarkInput {
	kind: string;
	/** An artifact's type: `Pagination`, `Layout`, `Page`, `Header`… */
	type?: string;
	lang?: string;
	alt?: string;
	expanded?: string;
	actual?: string;
}

/**
 * An outline entry; `parent` is the index of its parent entry, absent at
 * the top.
 */

export interface OutlineInput {
	title: string;
	page: number;
	x: number;
	y: number;
	open: boolean;
	parent?: number;
}

/**
 * One structure element; `parent` is the index of its parent among the
 * elements, or absent for a root.
 */

export interface StructureNodeInput {
	kind: string;
	title?: string;
	lang?: string;
	alt?: string;
	expanded?: string;
	actual?: string;
	scope?: string;
	rowSpan?: number;
	colSpan?: number;
}

/**
 * A child of element `parent`, in order: an element, marked content, or a
 * link.
 */

export interface StructureChildInput {
	parent: number;
	node?: number;
	content?: number;
	annotation?: number;
}

/** A file to embed. */

export interface FileInput {
	name: string;
	data: Buffer;
	mime?: string;
	description?: string;
	/** `Alternative`, `Data`, `Source`, `Supplement`, `Unspecified`. */
	relationship?: string;
	/** Milliseconds since the Unix epoch. */
	created?: number;
	modified?: number;
	hidden?: boolean;
}

/**
 * An annotation other than a link: `note`, `highlight`, `underline`,
 * `strike`, `line`, `square`, `circle`, `text`, `file`.
 */

export interface AnnotationInput {
	kind: string;
	x: number;
	y: number;
	width: number;
	height: number;
	contents?: string;
	/** `[r, g, b]`, 0 to 255. */
	colour?: Array<number>;
	/** A note's icon. */
	icon?: string;
	/** A line's end, the start being `x`, `y`. */
	x2?: number;
	y2?: number;
	/** A file annotation's file. */
	file?: number;
}

/** pdfkit's `format` of a form field. */

export interface FormatInput {
	/**
	 * `zip`, `zipPlus4`, `zip4`, `phone`, `ssn`, `date`, `time`, `number`,
	 * `percent`.
	 */
	type: string;
	param?: string;
	nDec?: number;
	sepComma?: boolean;
	negStyle?: string;
	currency?: string;
	currencyPrepend?: boolean;
}

export interface FieldInput {
	name: string;
	/**
	 * `text`, `pushButton`, `radioButton`, `checkbox`, `combo`, `list`;
	 * absent groups fields.
	 */
	kind?: string;
	parent?: number;
	flags?: number;
	/** `left`, `center`, `right`. */
	align?: string;
	value?: string;
	defaultValue?: string;
	choices?: Array<string>;
	label?: string;
	background?: Array<number>;
	border?: Array<number>;
	fontSize?: number;
	format?: FormatInput;
	onState?: string;
}

export interface RectInput {
	x: number;
	y: number;
	width: number;
	height: number;
}

export interface PermissionsInput {
	/** `lowResolution` or `highResolution`. */
	printing?: string;
	modifying?: boolean;
	copying?: boolean;
	annotating?: boolean;
	fillingForms?: boolean;
	contentAccessibility?: boolean;
	documentAssembly?: boolean;
}

export interface EncryptionInput {
	userPassword?: string;
	ownerPassword?: string;
	permissions?: PermissionsInput;
}

export interface ImageInfo {
	handle: number;
	/** Pixels, as stored. */
	width: number;
	height: number;
	/** EXIF orientation, 1 to 8. */
	orientation: number;
}

export interface TextBounds {
	width: number;
	height: number;
}

export interface PageRange {
	start: number;
	count: number;
}

/** `text()`'s options once the caller has resolved them. */

export interface TextOptionsInput {
	width?: number;
	height?: number;
	/**
	 * `left`, `center`, `right`, `justify`; absent follows the paragraph's
	 * direction.
	 */
	align?: string;
	lineGap?: number;
	paragraphGap?: number;
	indent?: number;
	indentAllLines?: boolean;
	columns?: number;
	columnGap?: number;
	ellipsis?: string;
	continued?: boolean;
	characterSpacing?: number;
	wordSpacing?: number;
	horizontalScaling?: number;
	features?: Array<string>;
	/** A named baseline: `top`, `alphabetic`, `middle`, `svg-middle`… */
	baseline?: string;
	/** A numeric baseline, in points. */
	baselineOffset?: number;
	/** Degrees, counter-clockwise. */
	rotation?: number;
	/** The skew of a fake italic. */
	oblique?: number;
	fill?: boolean;
	stroke?: boolean;
	underline?: boolean;
	strike?: boolean;
	/** A URL to link the text to. */
	link?: string;
	/** A page to link the text to, from zero. */
	linkPage?: number;
	goTo?: string;
	destination?: string;
	/** Mark each paragraph as structure content. */
	structure?: boolean;
}

/**
 * A colour or paint: `rgb` (0–255 each), `cmyk` (0–255 each), or a
 * `spot` / `gradient` handle.
 */

export interface PaintInput {
	kind: string;
	values?: Array<number>;
	handle?: number;
}

export interface GradientStopInput {
	offset: number;
	/** `rgb` or `cmyk`. */
	kind: string;
	values: Array<number>;
	opacity: number;
}

export interface GradientInput {
	/** `linear` (`x1 y1 x2 y2`) or `radial` (`x1 y1 r1 x2 y2 r2`). */
	kind: string;
	coords: Array<number>;
	stops: Array<GradientStopInput>;
	transform: Array<number>;
}

export interface DocumentInfo {
	pageCount: number;
	version: string;
	encrypted: boolean;
}

export interface PageSize {
	/** Width in points (72 per inch). A4 is 595.28. */
	width: number;
	/** Height in points. A4 is 841.89. */
	height: number;
}

/** A rectangle of a page, in points from the top-left corner. */

export interface Band {
	x: number;
	y: number;
	width: number;
	height: number;
}

/** Rasterising options, as a plain JavaScript object. */

export interface RenderOptions {
	/**
	 * Multiplier over the page's natural size, 1 being 72 DPI. Default 1.
	 * Ignored when `width` is given.
	 */
	scale?: number;
	/** Target width in pixels. Wins over `scale`. */
	width?: number;
	/** `"png"` (default) or `"jpeg"`. */
	format?: string;
	/** JPEG quality, 1-100. Only valid alongside `format: "jpeg"`. */
	quality?: number;
	/** `#rgb`, `#rrggbb`, `#rrggbbaa` or `"transparent"`. Default opaque white. */
	background?: string;
	/**
	 * Render only this rectangle, in points from the TOP-left corner — the
	 * same corner `stampText` measures from. A band that does not fit the
	 * page is an error, never a silent full page.
	 */
	band?: Band;
	/**
	 * The most pixels one page may rasterise to. 50 million by default —
	 * room for A4 at 600 DPI. A page declares its own size, so without a
	 * ceiling a document alone could ask for gigabytes.
	 */
	maxPixels?: number;
}

export interface PageDimensions {
	/** Width in points, before scaling. */
	width: number;
	/** Height in points, before scaling. */
	height: number;
}

/** What the `/Info` dictionary says about a document. */

export interface DocumentMetadata {
	title?: string;
	author?: string;
	subject?: string;
	keywords?: string;
	/** The application that authored the content. */
	creator?: string;
	/** The application that wrote the PDF. */
	producer?: string;
	/**
	 * ISO 8601 when the producer wrote a conforming date, otherwise the raw
	 * string it did write.
	 */
	createdAt?: string;
	modifiedAt?: string;
}

/** Where and how an image is laid onto a page. */

export interface StampOptions {
	/** Which page, counting from zero. Absent stamps every page. */
	page?: number;
	/** Points from the left edge. Default 0. */
	x?: number;
	/** Points from the TOP edge. Default 0. */
	y?: number;
	/** Drawn width in points. With `height` absent, the ratio is kept. */
	width?: number;
	/** Drawn height in points. With `width` absent, the ratio is kept. */
	height?: number;
	/** 0 is invisible, 1 is opaque. Default 1. */
	opacity?: number;
}

/** Where and how a line of text is written onto a page. */

export interface TextStampOptions {
	/** Which page, counting from zero. Absent writes on every page. */
	page?: number;
	/** Points from the left edge. Default 0. */
	x?: number;
	/** Points from the TOP edge, to the text's baseline. Default 0. */
	y?: number;
	/** Type size in points. Default 12. */
	size?: number;
	/**
	 * One of the 14 standard fonts, e.g. `"Helvetica"`, `"Times-Roman"`.
	 * Ignored when `fontData` is given.
	 */
	font?: string;
	/**
	 * A TrueType or OpenType file to embed, subsetted to the text. Lifts the
	 * WinAnsi limit of the standard fonts, at the cost of carrying the glyphs
	 * in the document.
	 */
	fontData?: Buffer;
	/** `#rgb` or `#rrggbb`. Default black. */
	color?: string;
	/** 0 is invisible, 1 is opaque. Default 1. */
	opacity?: number;
}

/** One interactive field of a document's form. */

export interface FormField {
	/**
	 * The fully qualified name — every ancestor's partial name joined with
	 * dots. This is the name used to fill the field in.
	 */
	name: string;
	/**
	 * `"text"`, `"checkbox"`, `"radio"`, `"pushButton"`, `"dropdown"`,
	 * `"listBox"` or `"signature"`.
	 */
	kind: string;
	value?: string;
	/**
	 * What a choice field offers, or the states a checkbox and radio accept.
	 * A checkbox's "on" state is chosen by the document, so ticking it means
	 * writing one of these.
	 */
	options: Array<string>;
	readOnly: boolean;
	required: boolean;
	multiline: boolean;
	password: boolean;
	maxLength?: number;
}

/** What the signature says about itself. */

export interface SignatureOptions {
	/** Why the document was signed. */
	reason?: string;
	/** Where it was signed. */
	location?: string;
	/** How to reach the signatory. */
	contact?: string;
	/** Who signed, as it should be displayed. */
	name?: string;
	/** When, as an ISO 8601 instant. */
	signedAt?: string;
	/** Bytes reserved for the signature value. Default 16384. */
	capacity?: number;
}

/** A document with room for a signature, and the digest to sign. */

export interface PreparedSignature {
	document: Buffer;
	/** SHA-256 of everything the signature covers. */
	digest: Buffer;
}

/** A query for a timestamp authority, and the nonce it has to echo back. */

export interface TimestampQuery {
	/** The DER to post to the authority. */
	query: Buffer;
	/** Opaque: hand it back to `attachTimestamp` unchanged. */
	nonce: Buffer;
}

/** What one signature on a document turns out to be. */

export interface SignatureReport {
	/** The field the signature sits in. */
	field: string;
	/** The signed range runs to the last byte, so nothing was appended after. */
	coversWholeDocument: boolean;
	/** The document's bytes hash to what the signature committed to. */
	digestMatches: boolean;
	/** The signature verifies against the certificate it carries. */
	signatureVerifies: boolean;
	/** Who the certificate says signed. */
	signer?: string;
	/** When the signature says it was made. */
	signedAt?: string;
	/** An authority has vouched for when, so it outlives the certificate. */
	timestamped: boolean;
	/** A path was found from the signer's certificate to a trusted anchor. */
	trusted: boolean;
	/** That path, the signer first and the anchor last. */
	chain: Array<string>;
	/**
	 * Where the instant used to judge the path came from: `"timestamp"`,
	 * `"claimed"` or `"unknown"`.
	 */
	moment: string;
	/** That instant, in seconds since the epoch. */
	momentAt?: number;
	/** The certificate that signed, DER. */
	signerCertificate?: Buffer;
	/** The certificate that issued it, DER — who answers about revocation. */
	issuerCertificate?: Buffer;
	/** Everything that could not be checked, or checked out wrong. */
	problems: Array<string>;
}

/** What a caller is willing to believe. */

export interface TrustOptions {
	/**
	 * Certificates to trust as roots, DER or PEM. Without them nothing can be
	 * trusted, which is what the report will say.
	 */
	anchors?: Array<Buffer>;
}

/** What a responder's answer says. */

export interface RevocationAnswer {
	/** `"good"`, `"revoked"` or `"unknown"`. */
	status: string;
	/** When it was withdrawn, or why nobody could be believed. */
	detail?: string;
}

export declare class NativeDocument {
	constructor(settings: DocumentSettingsInput);
	addPage(setup: PageSetupInput): void;
	setMargins(
		page: number,
		top: number,
		right: number,
		bottom: number,
		left: number,
	): void;
	getX(): number;
	setX(x: number): void;
	getY(): number;
	setY(y: number): void;
	bufferedPageRange(): PageRange;
	/** The page drawing goes to, counted from zero. */
	currentPage(): number | null;
	switchToPage(index: number): void;
	flushPages(): void;
	/**
	 * Load a font and answer its handle; `standard` names the standard
	 * face the bytes stand in for.
	 */
	loadFont(
		bytes: Buffer,
		family?: string | undefined | null,
		standard?: string | undefined | null,
	): number;
	setFont(handle: number): void;
	getFontSize(): number;
	setFontSize(size: number): void;
	getLineGap(): number;
	setLineGap(gap: number): void;
	/** The current font's ascender, in thousandths of an em. */
	ascender(): number;
	currentLineHeight(includeGap?: boolean | undefined | null): number;
	widthOfString(
		text: string,
		characterSpacing?: number | undefined | null,
		horizontalScaling?: number | undefined | null,
		features?: Array<string> | undefined | null,
	): number;
	/** `done`, or `needPage`: add a page, then [`Self::resume_text`]. */
	text(text: string, options: TextOptionsInput): string;
	/** One item of a list: `done`, or `needPage` as for [`Self::text`]. */
	listItem(
		text: string,
		options: TextOptionsInput,
		item: ListItemInput,
	): string;
	resumeText(): string;
	heightOfString(text: string, options: TextOptionsInput): number;
	/** Width and height of what `text()` would draw, before any rotation. */
	boundsOfString(text: string, options: TextOptionsInput): TextBounds;
	moveDown(lines: number): void;
	moveUp(lines: number): void;
	setInfo(info: DocumentInfoInput): void;
	save(): void;
	restore(): void;
	transform(values: Array<number>): void;
	getCtm(): Array<number>;
	setFillPaint(input: PaintInput): void;
	setStrokePaint(input: PaintInput): void;
	setFillOpacity(value: number): void;
	setStrokeOpacity(value: number): void;
	setLineWidth(width: number): void;
	/** `butt`, `round`, `square`. */
	setLineCap(cap: string): void;
	/** `miter`, `round`, `bevel`. */
	setLineJoin(join: string): void;
	setMiterLimit(limit: number): void;
	/** No lengths: a solid line. */
	setDash(
		lengths?: Array<number> | undefined | null,
		phase?: number | undefined | null,
	): void;
	addGradient(input: GradientInput): number;
	/** A spot colour falling back to CMYK (each 0–255). */
	addSpotColour(name: string, cmyk: Array<number>): number;
	/** `fill`: a fill rule, or absent to only stroke. */
	paintPath(
		verbs: string,
		coords: Array<number>,
		fill: string | undefined | null,
		stroke: boolean,
	): void;
	clipPath(
		verbs: string,
		coords: Array<number>,
		rule?: string | undefined | null,
	): void;
	/**
	 * A link over a rectangle: to a URL, a page (from zero), or a named
	 * destination.
	 */
	link(
		x: number,
		y: number,
		width: number,
		height: number,
		url?: string | undefined | null,
		page?: number | undefined | null,
		name?: string | undefined | null,
	): number;
	addNamedDestination(name: string, x: number, y: number): void;
	loadImage(bytes: Buffer): ImageInfo;
	/** Draw a loaded image into a box, turned by an EXIF orientation. */
	drawImage(
		handle: number,
		x: number,
		y: number,
		width: number,
		height: number,
		orientation: number,
		opacity?: number | undefined | null,
	): void;
	/** Begin a marked sequence and answer its handle. */
	beginMark(input: MarkInput): number;
	continueMark(id: number): void;
	endMark(): void;
	getOpenMark(): number | null;
	takeMarkedContents(): Array<number>;
	/** The structure tree, as elements and their children in order. */
	setStructure(
		nodes: Array<StructureNodeInput>,
		children: Array<StructureChildInput>,
	): void;
	/** The outline, entries in order, each after its parent. */
	setOutline(entries: Array<OutlineInput>): void;
	attachFile(input: FileInput): number;
	annotate(input: AnnotationInput): void;
	/**
	 * Open a named destination with another view: `Fit`, `FitB`, `FitH`,
	 * `FitBH` (top), `FitV`, `FitBV` (left), `FitR` (left, top, right,
	 * bottom), in user space.
	 */
	setDestinationFit(
		name: string,
		fit: string,
		values: Array<number | undefined | null>,
	): void;
	/** A form field, shown on the current page when `rect` is given. */
	formField(input: FieldInput, rect?: RectInput | undefined | null): number;
	setEncryption(input: EncryptionInput): void;
	/** Write the document. Nothing can be added afterwards. */
	end(): Buffer;
}

/** The file in the package's `fonts/` directory a standard name stands for. */

export declare function standardFontFile(name: string): string | null;

export declare function inspect(bytes: Buffer): DocumentInfo;

export declare function createBlank(pages: Array<PageSize>): Buffer;

export declare function renderPage(
	bytes: Buffer,
	pageIndex: number,
	options?: RenderOptions | undefined | null,
): Promise<Buffer>;

export declare function renderAll(
	bytes: Buffer,
	options?: RenderOptions | undefined | null,
): Promise<Buffer[]>;

export declare function pageDimensions(bytes: Buffer): Array<PageDimensions>;

export declare function metadata(bytes: Buffer): DocumentMetadata;

export declare function extractText(
	bytes: Buffer,
	pageIndex: number,
): Promise<string>;

export declare function extractTextAll(bytes: Buffer): Promise<string[]>;

export declare function merge(documents: Array<Buffer>): Promise<Buffer>;

export declare function selectPages(
	bytes: Buffer,
	pages: Array<number>,
): Promise<Buffer>;

export declare function split(bytes: Buffer): Promise<Buffer[]>;

export declare function rotate(
	bytes: Buffer,
	degrees: number,
	pages?: Array<number> | undefined | null,
): Promise<Buffer>;

export declare function stamp(
	pdf: Buffer,
	image: Buffer,
	options?: StampOptions | undefined | null,
): Promise<Buffer>;

export declare function stampText(
	pdf: Buffer,
	text: string,
	options?: TextStampOptions | undefined | null,
): Promise<Buffer>;

export declare function formFields(bytes: Buffer): Array<FormField>;

/**
 * Fill the named fields. Keys are the fully qualified names `formFields`
 * reports.
 */

export declare function fillForm(
	pdf: Buffer,
	values: Record<string, string>,
): Promise<Buffer>;

/** Paint every field into the page and drop the interactive layer. */

export declare function flattenForm(pdf: Buffer): Promise<Buffer>;

/** Write a document with room for a signature, and say what has to be signed. */

export declare function prepareSignature(
	pdf: Buffer,
	options?: SignatureOptions | undefined | null,
): Promise<PreparedSignature>;

/** Put the signature value into the space that was reserved for it. */

export declare function embedSignature(
	prepared: Buffer,
	value: Buffer,
): Promise<Buffer>;

/**
 * Turn a digest into the CMS a PDF signature carries, with a key we hold.
 *
 * The key is PKCS#8 DER and the certificates are DER, the signer's first.
 */

export declare function signCms(
	digest: Buffer,
	key: Buffer,
	certificates: Array<Buffer>,
	signedAt: string,
): Promise<Buffer>;

/** Build the query to post to a timestamp authority. */

export declare function timestampQuery(cms: Buffer): TimestampQuery;

/** Attach the authority's answer to the signature. */

export declare function attachTimestamp(
	cms: Buffer,
	response: Buffer,
	nonce: Buffer,
): Buffer;

/**
 * Report on every signature the document carries.
 *
 * This establishes integrity and authorship, not trust: it does not ask
 * whether the certificate comes from an authority you accept, nor whether it
 * has been revoked.
 */

export declare function verifySignatures(
	pdf: Buffer,
	trust?: TrustOptions | undefined | null,
): Promise<SignatureReport[]>;

/** The responder a certificate names, if it names one. */

export declare function responderUrl(certificate: Buffer): string | null;

/** Build the question to post to a revocation responder. */

export declare function revocationQuery(
	certificate: Buffer,
	issuer: Buffer,
): Buffer;

/**
 * Read a responder's answer about a certificate.
 *
 * `at` is the instant the document was signed: a certificate withdrawn after
 * that does not taint what it signed before.
 */

export declare function readRevocation(
	response: Buffer,
	certificate: Buffer,
	issuer: Buffer,
	at?: number | undefined | null,
): RevocationAnswer;
