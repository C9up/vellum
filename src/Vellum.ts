/**
 * Vellum — the PDF service.
 *
 * Resolved from the container as `vellum`, or imported directly from
 * `@c9up/vellum/services/main`. Every method is asynchronous: rasterising is
 * real computation, and it runs on the libuv thread pool rather than on the
 * thread serving requests.
 */

import { readFile } from "node:fs/promises";

import { VellumError } from "./errors.js";
import type {
	DocumentInfo,
	DocumentMetadata,
	FormField,
	PageDimensions,
	RevocationAnswer,
	SignatureReport,
} from "./native.js";
import {
	embedSignatureNative,
	extractTextAllNative,
	extractTextNative,
	fillFormNative,
	flattenFormNative,
	formFieldsNative,
	inspectNative,
	mergeNative,
	metadataNative,
	pageDimensionsNative,
	prepareSignatureNative,
	readRevocationNative,
	renderAllNative,
	renderPageNative,
	responderUrlNative,
	revocationQueryNative,
	rotateNative,
	selectPagesNative,
	splitNative,
	stampNative,
	stampTextNative,
	verifySignaturesNative,
} from "./native.js";
import { mayAsk, type ResponderPolicy } from "./responder.js";

/** Image encodings a page can be rasterised to. */
export type ImageFormat = "png" | "jpeg";

/** Rendering defaults, set in `config/vellum.ts`. */
export interface VellumConfig {
	/** Default encoding. `"png"` unless set. */
	format?: ImageFormat;
	/** Default multiplier over the page's natural size, 1 being 72 DPI. */
	scale?: number;
	/** Default target width in pixels. Takes precedence over `scale`. */
	width?: number;
	/** Default JPEG quality, 1-100. */
	quality?: number;
	/** Default background: `#rgb`, `#rrggbb`, `#rrggbbaa` or `"transparent"`. */
	background?: string;
	/**
	 * The most pixels one page may rasterise to. 50 million by default — room
	 * for A4 at 600 DPI, and A3 at 400.
	 *
	 * A page declares its own size, so without a ceiling a document alone was
	 * enough to ask for gigabytes: bounding each side to 65535 still leaves 16
	 * GiB of RGBA between two of them. Raise it knowingly when you render
	 * something genuinely large.
	 */
	maxPixels?: number;
	/**
	 * Fonts to write text with, by the name a caller asks for them by. Values
	 * are paths to TrueType or OpenType files.
	 *
	 * ```ts
	 * fonts: { body: app.makePath('resources/fonts/Inter-Regular.ttf') }
	 * ```
	 *
	 * A name declared here is looked up before the standard fonts, so calling
	 * one `Helvetica` shadows the standard one — deliberately, since a project
	 * that ships its own Helvetica means that one.
	 */
	fonts?: Record<string, string>;
	/**
	 * Who may sign a document, by the name a caller asks for.
	 *
	 * ```ts
	 * signers: {
	 *   internal: myLocalSigner,
	 *   qualified: myProviderSigner,
	 * }
	 * ```
	 *
	 * A key held here and a key held by a certified provider are the same
	 * thing to this package, because a signer never sees the document — only
	 * the digest of it. Which one signs is therefore a line of configuration.
	 */
	signers?: Record<string, Signer>;
	/**
	 * Certificates to trust when checking a signature, DER or PEM.
	 *
	 * Typically the roots of the authorities your jurisdiction recognises,
	 * which supervisory bodies publish as a trusted list. Supplying none is a
	 * position too: every signature then comes back untrusted, which is the
	 * honest answer rather than a comfortable one.
	 */
	trustedAnchors?: ReadonlyArray<Buffer>;
	/**
	 * Which revocation responders may be contacted.
	 *
	 * The address comes out of the certificate inside the document being
	 * checked — from whoever sent it. Left unset, only public hosts over
	 * http/https are asked, which is what a real certificate authority is. Set
	 * it to a list of hostnames, or a predicate, when your authority answers
	 * somewhere those rules exclude.
	 */
	allowedResponders?: ResponderPolicy;
}

/**
 * Whatever turns a digest into a signature.
 *
 * It is given the SHA-256 of the byte range the signature covers, and returns
 * the CMS `SignedData` to put in the document. It is never given the document:
 * a PDF signature covers a byte range of the file it lives in, so the value
 * has to be computed over a digest and dropped into space reserved for it.
 *
 * That is what lets a key in a file and a certified provider's API be the same
 * interface — and why signing over the network belongs here rather than in the
 * engine, which does no I/O.
 */
export interface Signer {
	sign(digest: Buffer): Promise<Buffer>;
}

/**
 * A signature report, plus what the issuer said about the certificate when
 * `checkRevocation` asked.
 */
export type CheckedSignature = SignatureReport & {
	revocation?: RevocationAnswer;
};

/** What to check a signature against. */
export interface VerifyOptions {
	/**
	 * Certificates to trust as roots, DER or PEM. Falls back to
	 * `trustedAnchors` in `config/vellum.ts`.
	 */
	anchors?: ReadonlyArray<Buffer>;
	/** Which responders may be contacted, overriding `config/vellum.ts`. */
	allowedResponders?: ResponderPolicy;
	/**
	 * Ask each certificate's issuer whether it still stands.
	 *
	 * A network call per signature, to the responder the certificate names.
	 * The answer has **three** values, not two: `"good"`, `"revoked"`, and
	 * `"unknown"` for everything else — the responder was unreachable,
	 * answered about something else, or could not be believed. Treating
	 * `"unknown"` as good waves through a withdrawn certificate; treating it
	 * as revoked rejects documents whenever a server is down. Which to do is
	 * your policy, so it is reported rather than decided.
	 */
	checkRevocation?: boolean;
	/** How long to wait on a responder. Default 10 seconds. */
	revocationTimeoutMs?: number;
}

/** What the signature says about itself. */
export interface SignOptions {
	/** Which signer, by the name it has in `config/vellum.ts`. */
	signer: string;
	/** Why the document was signed. */
	reason?: string;
	/** Where it was signed. */
	location?: string;
	/** How to reach the signatory. */
	contact?: string;
	/** Who signed, as it should be displayed. */
	name?: string;
	/** When. Defaults to now. */
	signedAt?: Date;
	/**
	 * Bytes reserved for the signature value. Default 16384, comfortable for a
	 * timestamped signature. The room cannot be found afterwards, so a signer
	 * that returns more than fits has to be given more here.
	 */
	capacity?: number;
}

/**
 * One of the 14 fonts every PDF reader is required to have.
 *
 * They can be referenced without being embedded, which is why writing text
 * adds nothing to the file and needs no font to be supplied.
 */
export type StandardFont =
	| "Helvetica"
	| "Helvetica-Bold"
	| "Helvetica-Oblique"
	| "Times-Roman"
	| "Times-Bold"
	| "Times-Italic"
	| "Courier"
	| "Courier-Bold";

/** Where and how a line of text is written onto a page. */
export interface TextStampOptions extends PageOptions {
	/** Points from the left edge. Default 0. */
	x?: number;
	/**
	 * Points from the top edge, to the text's BASELINE — the line the letters
	 * sit on, not the top of their bounding box.
	 */
	y?: number;
	/** Type size in points. Default 12. */
	size?: number;
	/**
	 * A font named in `config/vellum.ts`, or one of the 14 standard fonts.
	 * Default `"Helvetica"`.
	 */
	// The intersection keeps the standard names suggested while still
	// accepting a configured one.
	font?: StandardFont | (string & {});
	/** `#rgb` or `#rrggbb`. Default black. */
	color?: string;
	/** 0 is invisible, 1 is opaque. */
	opacity?: number;
}

/** Where and how an image is laid onto a page. */
export interface StampOptions extends PageOptions {
	/**
	 * Points from the left edge. Default 0.
	 *
	 * Coordinates count from the TOP-LEFT corner, the way a screen layout is
	 * written.
	 */
	x?: number;
	/** Points from the top edge. Default 0. */
	y?: number;
	/** Drawn width in points. With `height` absent, the ratio is kept. */
	width?: number;
	/** Drawn height in points. With `width` absent, the ratio is kept. */
	height?: number;
	/** 0 is invisible, 1 is opaque. A watermark usually wants about 0.15. */
	opacity?: number;
}

/** Options that address a single page. */
export interface PageOptions {
	/**
	 * Which page, counting from 1 — the number printed on the page, not an
	 * array index.
	 */
	page?: number;
}

/** Per-call rendering options. Anything omitted falls back to the config. */
export interface RenderOptions extends VellumConfig, PageOptions {}

export class Vellum {
	readonly #config: VellumConfig;
	/** Configured fonts, read once each. They do not change under us. */
	readonly #fonts = new Map<string, Buffer>();

	constructor(config: VellumConfig = {}) {
		this.#config = config;
	}

	/**
	 * The bytes of a configured font, or `undefined` when the name is not one.
	 *
	 * A name that is not configured falls through to the standard fonts, which
	 * is what makes `font: 'Times-Roman'` keep working with no configuration
	 * at all.
	 */
	async #font(name: string | undefined): Promise<Buffer | undefined> {
		if (name === undefined) return undefined;
		const path = this.#config.fonts?.[name];
		if (path === undefined) return undefined;

		const loaded = this.#fonts.get(path);
		if (loaded !== undefined) return loaded;

		try {
			const data = await readFile(path);
			this.#fonts.set(path, data);
			return data;
		} catch (error) {
			throw new VellumError(
				"FONT_UNREADABLE",
				`The font ${name} is configured as ${path}, which cannot be read.`,
				{ cause: error },
			);
		}
	}

	/** The configured defaults, as the service resolved them. */
	get config(): Readonly<VellumConfig> {
		return this.#config;
	}

	/**
	 * Rasterise a single page to an image.
	 *
	 * ```ts
	 * const preview = await vellum.render(pdf, { page: 1, width: 1200 })
	 * ```
	 */
	async render(pdf: Buffer, options: RenderOptions = {}): Promise<Buffer> {
		return renderPageNative(
			pdf,
			this.#pageIndex(options),
			this.#merge(options),
		);
	}

	/**
	 * Rasterise every page, in document order.
	 *
	 * ```ts
	 * const pages = await vellum.renderAll(pdf, { format: 'jpeg' })
	 * ```
	 */
	async renderAll(pdf: Buffer, options: RenderOptions = {}): Promise<Buffer[]> {
		return renderAllNative(pdf, this.#merge(options));
	}

	/** The natural size of every page, in points, before any scaling. */
	async dimensions(pdf: Buffer): Promise<PageDimensions[]> {
		return pageDimensionsNative(pdf);
	}

	/** How many pages the document has, which format version, and whether it is encrypted. */
	async inspect(pdf: Buffer): Promise<DocumentInfo> {
		return inspectNative(pdf);
	}

	/**
	 * What the document says about itself: title, author, subject, keywords,
	 * the applications involved, and the dates.
	 *
	 * Every field is optional, because a PDF is valid with no `/Info` at all
	 * and producers fill in whichever ones they like. Dates come back as ISO
	 * 8601 when the producer wrote a conforming one, otherwise as the raw
	 * string it did write.
	 */
	async metadata(pdf: Buffer): Promise<DocumentMetadata> {
		return metadataNative(pdf);
	}

	/**
	 * The text of a single page.
	 *
	 * ```ts
	 * const text = await vellum.extractText(pdf, { page: 1 })
	 * ```
	 *
	 * Glyphs come back in the order the page draws them, with a line break
	 * where the baseline moves. A scanned document with no text layer yields
	 * an empty string rather than an error — it has no text to give.
	 */
	async extractText(pdf: Buffer, options: PageOptions = {}): Promise<string> {
		return extractTextNative(pdf, this.#pageIndex(options));
	}

	/** The text of every page, in document order. */
	async extractTextAll(pdf: Buffer): Promise<string[]> {
		return extractTextAllNative(pdf);
	}

	/**
	 * Join documents end to end, in the order given.
	 *
	 * ```ts
	 * const dossier = await vellum.merge([contract, annexe, signature])
	 * ```
	 *
	 * Attributes a page inherits from its parent — size, resources — are
	 * materialised onto it first, so a page keeps its own size instead of
	 * falling back to Letter.
	 */
	async merge(pdfs: ReadonlyArray<Buffer>): Promise<Buffer> {
		if (pdfs.length === 0) {
			throw new VellumError(
				"EMPTY_MERGE",
				"Merging needs at least one document.",
			);
		}
		return mergeNative(pdfs);
	}

	/**
	 * Keep only the pages listed, counting from 1, in document order.
	 *
	 * ```ts
	 * const extract = await vellum.selectPages(pdf, [1, 3, 4])
	 * ```
	 */
	async selectPages(
		pdf: Buffer,
		pages: ReadonlyArray<number>,
	): Promise<Buffer> {
		if (pages.length === 0) {
			throw new VellumError(
				"EMPTY_SELECTION",
				"Selecting needs at least one page.",
			);
		}
		return selectPagesNative(pdf, this.#pageIndexes(pages));
	}

	/** One single-page document per page, in document order. */
	async split(pdf: Buffer): Promise<Buffer[]> {
		return splitNative(pdf);
	}

	/**
	 * Rotate pages clockwise by `degrees`, a multiple of 90.
	 *
	 * ```ts
	 * const upright = await vellum.rotate(scan, 90, { pages: [1] })
	 * ```
	 *
	 * The rotation is added to what a page already carries, because a scan can
	 * arrive already turned.
	 */
	async rotate(
		pdf: Buffer,
		degrees: number,
		options: { pages?: ReadonlyArray<number> } = {},
	): Promise<Buffer> {
		if (!Number.isInteger(degrees) || degrees % 90 !== 0) {
			throw new VellumError(
				"INVALID_ROTATION",
				`Rotation must be a whole multiple of 90, got ${degrees}.`,
			);
		}
		return rotateNative(
			pdf,
			degrees,
			options.pages ? this.#pageIndexes(options.pages) : undefined,
		);
	}

	/**
	 * Draw an image onto the document — a signature, a photo, a watermark.
	 *
	 * ```ts
	 * const signed = await vellum.stamp(workOrder, signature, {
	 *   page: 1, x: 380, y: 690, width: 140,
	 * })
	 * ```
	 *
	 * PNG and JPEG are accepted, chosen by signature rather than by file name.
	 * Omitting `page` stamps every page, which is what a watermark wants.
	 */
	async stamp(
		pdf: Buffer,
		image: Buffer,
		options: StampOptions = {},
	): Promise<Buffer> {
		return stampNative(pdf, image, {
			// Absent means every page here, so the 1-based conversion only
			// applies when a page was actually named.
			page: options.page === undefined ? undefined : this.#pageIndex(options),
			x: options.x,
			y: options.y,
			width: options.width,
			height: options.height,
			opacity: options.opacity,
		});
	}

	/**
	 * Write a line of text onto the document.
	 *
	 * ```ts
	 * const marked = await vellum.stampText(invoice, 'PAYÉ', {
	 *   page: 1, x: 400, y: 80, size: 24, color: '#c00', opacity: 0.6,
	 * })
	 * ```
	 *
	 * `font` names one of the 14 standard fonts by default. A PDF may
	 * reference those without embedding them, so nothing is added to the file
	 * — at the cost of the WinAnsi character set, outside which text is
	 * refused rather than mangled.
	 *
	 * Naming a font declared in `config/vellum.ts` instead embeds it,
	 * subsetted to the characters actually written, and lifts that limit:
	 *
	 * ```ts
	 * // config/vellum.ts
	 * fonts: { body: app.makePath('resources/fonts/Inter-Regular.ttf') }
	 *
	 * await vellum.stampText(pdf, 'Uměl Řehoř', { font: 'body' })
	 * ```
	 *
	 * Naming no page writes on every page, which is what a draft marking
	 * wants.
	 */
	async stampText(
		pdf: Buffer,
		text: string,
		options: TextStampOptions = {},
	): Promise<Buffer> {
		const fontData = await this.#font(options.font);
		return stampTextNative(pdf, text, {
			page: options.page === undefined ? undefined : this.#pageIndex(options),
			x: options.x,
			y: options.y,
			size: options.size,
			font: fontData === undefined ? options.font : undefined,
			fontData,
			color: options.color,
			opacity: options.opacity,
		});
	}

	/**
	 * The interactive fields of the document's form, in declaration order.
	 *
	 * ```ts
	 * for (const field of await vellum.formFields(mandate)) {
	 *   console.log(field.name, field.kind, field.value)
	 * }
	 * ```
	 *
	 * `name` is the fully qualified name — the one used to fill the field in.
	 * For a checkbox or a radio group, `options` lists the states the DOCUMENT
	 * accepts: their "on" state is not a fixed name, and writing anything else
	 * leaves the control untouched.
	 *
	 * A document with no form yields an empty list rather than an error.
	 */
	async formFields(pdf: Buffer): Promise<FormField[]> {
		return formFieldsNative(pdf);
	}

	/**
	 * Fill the named fields of the document's form.
	 *
	 * ```ts
	 * const filled = await vellum.fillForm(mandate, {
	 *   'assure.nom': 'Amélie Durand',
	 *   accepted: 'Yes',
	 *   country: 'CH',
	 * })
	 * ```
	 *
	 * Keys are the fully qualified names {@link Vellum.formFields} reports.
	 *
	 * Each filled field's **appearance stream is regenerated**. Writing the
	 * value alone is not enough: most readers paint a field from its
	 * appearance, not from its value, so a document filled without that opens
	 * looking empty while holding every answer.
	 *
	 * A name the form does not have is an error rather than a silent no-op —
	 * a filled document missing an answer nobody noticed is worse than a
	 * failure. The same goes for a read-only field, a value over the field's
	 * maximum length, and a checkbox state the document does not accept.
	 */
	async fillForm(pdf: Buffer, values: Record<string, string>): Promise<Buffer> {
		return fillFormNative(pdf, values);
	}

	/**
	 * Paint the form into the page and remove it.
	 *
	 * ```ts
	 * const signed = await vellum.flattenForm(
	 *   await vellum.fillForm(mandate, { 'assure.nom': 'Amélie Durand' }),
	 * )
	 * ```
	 *
	 * The document keeps its look and loses its fields: every widget's
	 * appearance becomes ordinary page content, the widget annotations go, and
	 * the form itself is dropped. This is what turns a filled document into
	 * one nobody can edit back.
	 *
	 * Annotations that are not form widgets — links, notes — are left where
	 * they are. A field holding a value that ships no appearance to paint is
	 * an error: the answer would vanish from a document that still looks
	 * complete.
	 */
	async flattenForm(pdf: Buffer): Promise<Buffer> {
		return flattenFormNative(pdf);
	}

	/**
	 * Sign the document with one of the configured signers.
	 *
	 * ```ts
	 * const signed = await vellum.sign(mandate, {
	 *   signer: 'qualified',
	 *   reason: 'Mandat de prévoyance',
	 *   name: 'Amélie Durand',
	 * })
	 * ```
	 *
	 * The signature is appended as an **incremental revision**: the bytes it
	 * signs are preserved exactly, and nothing already in the document is
	 * rewritten. Rewriting would invalidate any signature already on it and
	 * destroy the history a signature exists to establish.
	 *
	 * The signer is handed the digest of what the signature covers and returns
	 * the CMS to embed. It never sees the document, which is what makes a
	 * local key and a certified provider interchangeable.
	 *
	 * A visible signature — a drawn one, an image — is a separate matter:
	 * {@link Vellum.stamp} it on first, then sign.
	 */
	async sign(pdf: Buffer, options: SignOptions): Promise<Buffer> {
		const signer = this.#config.signers?.[options.signer];
		if (signer === undefined) {
			const known = Object.keys(this.#config.signers ?? {});
			throw new VellumError(
				"UNKNOWN_SIGNER",
				`No signer named ${options.signer} is configured — ` +
					(known.length === 0
						? "config/vellum.ts declares none."
						: `config/vellum.ts declares ${known.join(", ")}.`),
			);
		}

		const prepared = await prepareSignatureNative(pdf, {
			reason: options.reason,
			location: options.location,
			contact: options.contact,
			name: options.name,
			signedAt: (options.signedAt ?? new Date()).toISOString(),
			capacity: options.capacity,
		});

		const value = await signer.sign(prepared.digest);
		return embedSignatureNative(prepared.document, value);
	}

	/**
	 * Report on every signature the document carries.
	 *
	 * ```ts
	 * for (const signature of await vellum.verifySignatures(mandate)) {
	 *   if (!signature.coversWholeDocument) reject('content was added after signing')
	 *   if (!signature.digestMatches) reject('the document has changed')
	 * }
	 * ```
	 *
	 * `coversWholeDocument` is the one that catches the trap everybody meets
	 * first: content appended after a signature is not covered by it, and the
	 * arithmetic over the covered part still checks out. A document whose
	 * second half arrived later is not a signed document.
	 *
	 * This establishes **integrity and authorship, not trust**. It does not ask
	 * whether the certificate comes from an authority you accept, nor whether
	 * it has since been revoked: that needs a trust store and a live revocation
	 * check, neither of which belongs in a PDF engine.
	 *
	 * `trusted` says a path was found from the signer's certificate to one of
	 * the anchors you accept, judged **at the moment of signing** rather than
	 * now — a certificate that has since expired did not retroactively unsign
	 * anything. `moment` says where that instant came from: a timestamp is
	 * worth having because it makes it something other than the signer's word.
	 *
	 * **Revocation is not checked** unless `checkRevocation` asks for it. A
	 * certificate withdrawn after it was issued otherwise still looks valid
	 * here.
	 *
	 * When it is asked for, the responder's address comes out of the
	 * certificate inside the document — from whoever sent it. Only public hosts
	 * over http/https are contacted; a document does not get to point your
	 * server at your own network. Name your authority in `allowedResponders`
	 * when it answers somewhere that rule excludes.
	 *
	 * A document with no signatures reports none. That is an answer, not a
	 * failure.
	 */
	async verifySignatures(
		pdf: Buffer,
		options: VerifyOptions = {},
	): Promise<CheckedSignature[]> {
		const reports = await verifySignaturesNative(pdf, {
			anchors: [...(options.anchors ?? this.#config.trustedAnchors ?? [])],
		});
		if (options.checkRevocation !== true) return reports;

		return Promise.all(
			reports.map(async (report) => ({
				...report,
				revocation: await this.#revocationOf(
					report,
					options.revocationTimeoutMs ?? 10_000,
					options.allowedResponders ?? this.#config.allowedResponders,
				),
			})),
		);
	}

	/**
	 * Ask a certificate's issuer whether it still stands.
	 *
	 * Everything that can go wrong answers `"unknown"`, never `"good"`: a
	 * responder that cannot be reached has told us nothing, and pretending
	 * otherwise is how a withdrawn certificate gets waved through.
	 */
	async #revocationOf(
		report: SignatureReport,
		timeoutMs: number,
		policy: ResponderPolicy | undefined,
	): Promise<RevocationAnswer> {
		const { signerCertificate, issuerCertificate } = report;
		if (signerCertificate === undefined || issuerCertificate === undefined) {
			return {
				status: "unknown",
				detail:
					"the issuer of this certificate is not known, so nobody can be asked",
			};
		}

		const named = responderUrlNative(signerCertificate);
		if (named === null || named === undefined) {
			return {
				status: "unknown",
				detail: "the certificate names no responder to ask",
			};
		}

		// The address is the document's, not ours. Asking it unconditionally
		// turns this into "make my server issue a request wherever this stranger
		// points" — the cloud metadata endpoint, a service on loopback, or a URL
		// whose only purpose is to report that the document was opened.
		const allowed = await mayAsk(named, policy);
		if ("refused" in allowed) {
			return { status: "unknown", detail: allowed.refused };
		}
		const url = allowed.href;

		let answer: Response;
		try {
			answer = await fetch(url, {
				method: "POST",
				headers: { "content-type": "application/ocsp-request" },
				body: new Uint8Array(
					revocationQueryNative(signerCertificate, issuerCertificate),
				),
				signal: AbortSignal.timeout(timeoutMs),
			});
		} catch (error) {
			return {
				status: "unknown",
				detail: `the responder at ${url} could not be reached: ${
					error instanceof Error ? error.message : String(error)
				}`,
			};
		}
		if (!answer.ok) {
			return {
				status: "unknown",
				detail: `the responder at ${url} answered ${answer.status}`,
			};
		}

		return readRevocationNative(
			Buffer.from(await answer.arrayBuffer()),
			signerCertificate,
			issuerCertificate,
			report.momentAt ?? null,
		);
	}

	/** How many pages the document has. */
	async pageCount(pdf: Buffer): Promise<number> {
		return (await this.inspect(pdf)).pageCount;
	}

	/**
	 * Resolve a 1-based page number to the engine's 0-based index.
	 *
	 * Shared by every page-addressing method: converted in exactly one place
	 * so the public numbering and the engine's can never drift apart.
	 */
	#pageIndex(options: PageOptions): number {
		const page = options.page ?? 1;
		if (!Number.isInteger(page) || page < 1) {
			throw new VellumError(
				"INVALID_PAGE",
				`Page numbers start at 1, got ${page}.`,
			);
		}
		return page - 1;
	}

	/**
	 * Resolve several 1-based page numbers to the engine's 0-based indexes.
	 *
	 * Routed through {@link Vellum.#pageIndex} so one method decides what a
	 * page number means for the whole service.
	 */
	#pageIndexes(pages: ReadonlyArray<number>): number[] {
		return pages.map((page) => this.#pageIndex({ page }));
	}

	/**
	 * Fold the per-call options onto the configured defaults.
	 *
	 * `page` is dropped: it addresses the document, not the raster, and the
	 * engine takes it as a separate argument.
	 */
	#merge(options: RenderOptions): {
		scale?: number;
		width?: number;
		format?: string;
		quality?: number;
		background?: string;
		maxPixels?: number;
	} {
		const merged = { ...this.#config, ...options };
		return {
			scale: merged.scale,
			width: merged.width,
			format: merged.format,
			quality: merged.quality,
			background: merged.background,
			maxPixels: merged.maxPixels,
		};
	}
}
