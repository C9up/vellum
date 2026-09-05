// GENERATED FROM THE RUST — do not edit.
//
// Produced by scripts/generate-napi-types.mjs from napi-derive's type-def
// output. Editing this file by hand puts it back where it started: a
// description that can disagree with the code it describes.

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
