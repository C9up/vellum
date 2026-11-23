/**
 * Loader for the Rust PDF engine.
 *
 * The engine is not optional. Parsing, authoring and rendering PDF have no
 * JavaScript implementation in this package, so a missing binary is a hard
 * failure with an actionable message — never a silent degradation that lets
 * one deployment behave differently from another.
 */

import { type ErrorClass, Exception } from "@c9up/flare";
import {
	E_VELLUM_DOCUMENT_BROKEN,
	E_VELLUM_DOCUMENT_ENDED,
	E_VELLUM_EXTRACT_FAILED,
	E_VELLUM_FILL_FAILED,
	E_VELLUM_FLATTEN_FAILED,
	E_VELLUM_INTERNAL,
	E_VELLUM_INVALID_FONT,
	E_VELLUM_INVALID_IMAGE,
	E_VELLUM_INVALID_LINK,
	E_VELLUM_INVALID_OPTIONS,
	E_VELLUM_INVALID_OUTLINE,
	E_VELLUM_INVALID_PDF,
	E_VELLUM_INVALID_STRUCTURE,
	E_VELLUM_INVALID_SVG,
	E_VELLUM_MERGE_FAILED,
	E_VELLUM_NAPI_REQUIRED,
	E_VELLUM_NO_FONT,
	E_VELLUM_NO_PAGE,
	E_VELLUM_NOTHING_TO_RESUME,
	E_VELLUM_PAGE_OUT_OF_RANGE,
	E_VELLUM_RENDER_FAILED,
	E_VELLUM_ROTATE_FAILED,
	E_VELLUM_SELECT_FAILED,
	E_VELLUM_SIGN_FAILED,
	E_VELLUM_SIGNED_DOCUMENT,
	E_VELLUM_SPLIT_FAILED,
	E_VELLUM_STAMP_FAILED,
	E_VELLUM_STAMP_TEXT_FAILED,
	E_VELLUM_TIMESTAMP_FAILED,
	E_VELLUM_UNBALANCED_RESTORE,
	E_VELLUM_VALIDATION,
	E_VELLUM_VERIFY_FAILED,
	E_VELLUM_WRITE_FAILED,
} from "./errors.js";
import type {
	DocumentInfo,
	DocumentMetadata,
	DocumentSettingsInput,
	EditOptions,
	FormField,
	NativeDocument,
	SignatureOptions as NativeSignatureOptions,
	StampOptions as NativeStampOptions,
	TextStampOptions as NativeTextStampOptions,
	TrustOptions as NativeTrustOptions,
	PageDimensions,
	PageSize,
	PreparedSignature,
	ReadLimits,
	RenderOptions,
	RevocationAnswer,
	SignatureReport,
	TimestampQuery,
} from "./native/generated.js";
import {
	hasFunctions,
	loadNativeBinary,
	unavailableReason as vendorUnavailableReason,
} from "./vendor/nativeBinary.js";

/**
 * What this package calls on the engine, typed as the Rust declares it.
 *
 * Derived from `./native/generated.js` — written by `pnpm build:napi-types`
 * from napi-derive's own `type-def` output — rather than restated here, where
 * nothing would notice a `pub fn` gaining a parameter or changing its return.
 */
const NATIVE_CALLS = [
	"NativeDocument",
	"standardFontFile",
	"inspect",
	"createBlank",
	"renderPage",
	"renderAll",
	"pageDimensions",
	"metadata",
	"extractText",
	"extractTextAll",
	"merge",
	"selectPages",
	"split",
	"rotate",
	"stamp",
	"stampText",
	"formFields",
	"fillForm",
	"flattenForm",
	"prepareSignature",
	"embedSignature",
	"signCms",
	"timestampQuery",
	"attachTimestamp",
	"verifySignatures",
	"responderUrl",
	"revocationQuery",
	"readRevocation",
] as const;

type NativeVellum = Pick<
	typeof import("./native/generated.js"),
	(typeof NATIVE_CALLS)[number]
>;

/** What the package calls, checked at load: a stale binary loads just as well. */
function isNativeVellum(value: unknown): value is NativeVellum {
	return hasFunctions(value, NATIVE_CALLS);
}

export type {
	DocumentInfo,
	DocumentMetadata,
	FormField,
	PageDimensions,
	PageSize,
	PreparedSignature,
	RevocationAnswer,
	SignatureReport,
} from "./native/generated.js";

const attempt = loadNativeBinary(isNativeVellum, NATIVE_CALLS);
const native = attempt.loaded ? attempt.binary : undefined;
const loadError = attempt.loaded ? undefined : attempt.cause;

export function isNativeAvailable(): boolean {
	return native !== undefined;
}

/** Why the engine could not be loaded, phrased for whoever has to fix it. */
function unavailableReason(): string {
	if (loadError !== undefined) {
		return `failed to load (${loadError instanceof Error ? loadError.message : String(loadError)})`;
	}
	return vendorUnavailableReason();
}

/** The engine, or a refusal explaining how to get one. */
function engine(): NativeVellum {
	if (native === undefined) {
		throw new E_VELLUM_NAPI_REQUIRED([unavailableReason()]);
	}
	return native;
}

/** The message of whatever the engine threw. */
function messageOf(error: unknown): string {
	return error instanceof Error ? error.message : String(error);
}

/**
 * Run engine work, translating its failures into coded errors.
 *
 * The engine reports failures as plain reasons. Without this, a caller
 * catching a corrupt upload would get an `Error` it cannot branch on.
 */
function run<T>(
	failure: ErrorClass<[string]>,
	work: (engine: NativeVellum) => T,
): T {
	const loaded = engine();
	try {
		return work(loaded);
	} catch (error) {
		throw new failure([messageOf(error)], { cause: error });
	}
}

export function inspectNative(pdf: Buffer): Promise<DocumentInfo> {
	return edit(E_VELLUM_INVALID_PDF, (loaded) => loaded.inspect(pdf));
}

export function createBlankNative(pages: ReadonlyArray<PageSize>): Buffer {
	return run(E_VELLUM_WRITE_FAILED, (loaded) => loaded.createBlank([...pages]));
}

/**
 * Rasterise one page, addressed from zero.
 *
 * The engine hands this to the libuv thread pool, so the returned promise
 * settles without the calling thread ever blocking.
 */
export async function renderPageNative(
	pdf: Buffer,
	pageIndex: number,
	options: RenderOptions,
): Promise<Buffer> {
	const loaded = engine();
	try {
		return await loaded.renderPage(pdf, pageIndex, options);
	} catch (error) {
		throw new E_VELLUM_RENDER_FAILED([messageOf(error)], { cause: error });
	}
}

/** Rasterise every page, in document order. */
export async function renderAllNative(
	pdf: Buffer,
	options: RenderOptions,
): Promise<Buffer[]> {
	const loaded = engine();
	try {
		return await loaded.renderAll(pdf, options);
	} catch (error) {
		throw new E_VELLUM_RENDER_FAILED([messageOf(error)], { cause: error });
	}
}

export function pageDimensionsNative(
	pdf: Buffer,
	limits: ReadLimits,
): Promise<PageDimensions[]> {
	return edit(E_VELLUM_INVALID_PDF, (loaded) =>
		loaded.pageDimensions(pdf, limits),
	);
}

export function metadataNative(pdf: Buffer): Promise<DocumentMetadata> {
	return edit(E_VELLUM_INVALID_PDF, (loaded) => loaded.metadata(pdf));
}

export async function extractTextNative(
	pdf: Buffer,
	pageIndex: number,
	limits: ReadLimits,
): Promise<string> {
	const loaded = engine();
	try {
		return await loaded.extractText(pdf, pageIndex, limits);
	} catch (error) {
		throw new E_VELLUM_EXTRACT_FAILED([messageOf(error)], { cause: error });
	}
}

export async function extractTextAllNative(
	pdf: Buffer,
	limits: ReadLimits,
): Promise<string[]> {
	const loaded = engine();
	try {
		return await loaded.extractTextAll(pdf, limits);
	} catch (error) {
		throw new E_VELLUM_EXTRACT_FAILED([messageOf(error)], { cause: error });
	}
}

/** Failures the engine names itself, whatever the operation. */
const NAMED_FAILURE = /^(SIGNED_DOCUMENT): ([\s\S]*)$/;

async function edit<T>(
	failure: ErrorClass<[string]>,
	work: (engine: NativeVellum) => Promise<T>,
): Promise<T> {
	const loaded = engine();
	try {
		return await work(loaded);
	} catch (error) {
		const message = messageOf(error);
		const reason = NAMED_FAILURE.exec(message)?.[2];
		if (reason !== undefined) {
			throw new E_VELLUM_SIGNED_DOCUMENT([reason], { cause: error });
		}
		throw new failure([message], { cause: error });
	}
}

export function mergeNative(
	documents: ReadonlyArray<Buffer>,
	options: EditOptions,
): Promise<Buffer> {
	return edit(E_VELLUM_MERGE_FAILED, (loaded) =>
		loaded.merge([...documents], options),
	);
}

export function selectPagesNative(
	pdf: Buffer,
	pageIndexes: ReadonlyArray<number>,
	options: EditOptions,
): Promise<Buffer> {
	return edit(E_VELLUM_SELECT_FAILED, (loaded) =>
		loaded.selectPages(pdf, [...pageIndexes], options),
	);
}

export function splitNative(
	pdf: Buffer,
	options: EditOptions,
): Promise<Buffer[]> {
	return edit(E_VELLUM_SPLIT_FAILED, (loaded) => loaded.split(pdf, options));
}

export function rotateNative(
	pdf: Buffer,
	degrees: number,
	pageIndexes: ReadonlyArray<number> | undefined,
	options: EditOptions,
): Promise<Buffer> {
	return edit(E_VELLUM_ROTATE_FAILED, (loaded) =>
		loaded.rotate(
			pdf,
			degrees,
			pageIndexes ? [...pageIndexes] : undefined,
			options,
		),
	);
}

export function stampNative(
	pdf: Buffer,
	image: Buffer,
	options: NativeStampOptions,
): Promise<Buffer> {
	return edit(E_VELLUM_STAMP_FAILED, (loaded) =>
		loaded.stamp(pdf, image, options),
	);
}

export function stampTextNative(
	pdf: Buffer,
	text: string,
	options: NativeTextStampOptions,
): Promise<Buffer> {
	return edit(E_VELLUM_STAMP_TEXT_FAILED, (loaded) =>
		loaded.stampText(pdf, text, options),
	);
}

export function formFieldsNative(pdf: Buffer): Promise<FormField[]> {
	return edit(E_VELLUM_INVALID_PDF, (loaded) => loaded.formFields(pdf));
}

export function fillFormNative(
	pdf: Buffer,
	values: Record<string, string>,
	options: EditOptions,
): Promise<Buffer> {
	return edit(E_VELLUM_FILL_FAILED, (loaded) =>
		loaded.fillForm(pdf, values, options),
	);
}

export function flattenFormNative(
	pdf: Buffer,
	options: EditOptions,
): Promise<Buffer> {
	return edit(E_VELLUM_FLATTEN_FAILED, (loaded) =>
		loaded.flattenForm(pdf, options),
	);
}

export function prepareSignatureNative(
	pdf: Buffer,
	options: NativeSignatureOptions,
): Promise<PreparedSignature> {
	return edit(E_VELLUM_SIGN_FAILED, (loaded) =>
		loaded.prepareSignature(pdf, options),
	);
}

export function embedSignatureNative(
	prepared: Buffer,
	value: Buffer,
): Promise<Buffer> {
	return edit(E_VELLUM_SIGN_FAILED, (loaded) =>
		loaded.embedSignature(prepared, value),
	);
}

export function signCmsNative(
	digest: Buffer,
	key: Buffer,
	certificates: ReadonlyArray<Buffer>,
	signedAt: string,
): Promise<Buffer> {
	return edit(E_VELLUM_SIGN_FAILED, (loaded) =>
		loaded.signCms(digest, key, [...certificates], signedAt),
	);
}

export function timestampQueryNative(cms: Buffer): TimestampQuery {
	return run(E_VELLUM_TIMESTAMP_FAILED, (loaded) => loaded.timestampQuery(cms));
}

export function attachTimestampNative(
	cms: Buffer,
	response: Buffer,
	nonce: Buffer,
): Buffer {
	return run(E_VELLUM_TIMESTAMP_FAILED, (loaded) =>
		loaded.attachTimestamp(cms, response, nonce),
	);
}

export function verifySignaturesNative(
	pdf: Buffer,
	trust: NativeTrustOptions,
): Promise<SignatureReport[]> {
	return edit(E_VELLUM_VERIFY_FAILED, (loaded) =>
		loaded.verifySignatures(pdf, trust),
	);
}

export function responderUrlNative(certificate: Buffer): string | null {
	return run(E_VELLUM_VERIFY_FAILED, (loaded) =>
		loaded.responderUrl(certificate),
	);
}

export function revocationQueryNative(
	certificate: Buffer,
	issuer: Buffer,
): Buffer {
	return run(E_VELLUM_VERIFY_FAILED, (loaded) =>
		loaded.revocationQuery(certificate, issuer),
	);
}

export function readRevocationNative(
	response: Buffer,
	certificate: Buffer,
	issuer: Buffer,
	at: number | null,
): RevocationAnswer {
	return run(E_VELLUM_VERIFY_FAILED, (loaded) =>
		loaded.readRevocation(response, certificate, issuer, at ?? undefined),
	);
}

export type {
	DocumentSettingsInput,
	NativeDocument,
	PageSetupInput,
	TextOptionsInput,
} from "./native/generated.js";

/**
 * Turn a document failure into a coded error. The engine reports them as
 * `CODE: reason`.
 */
function composeFailure(error: unknown): Exception {
	if (error instanceof Exception) return error;
	const message = messageOf(error);
	const match = /^([A-Z][A-Z0-9_]*): ([\s\S]*)$/.exec(message);
	const code = match?.[1];
	const reason = match?.[2];
	if (code !== undefined && reason !== undefined && isComposeCode(code)) {
		return new COMPOSE_ERRORS[code]([reason], { cause: error });
	}
	// A code this side does not know yet: the whole message keeps it readable.
	return new E_VELLUM_WRITE_FAILED([message], { cause: error });
}

/** The error class of each code the engine names a document failure with. */
const COMPOSE_ERRORS = {
	INVALID_OPTIONS: E_VELLUM_INVALID_OPTIONS,
	WRITE_FAILED: E_VELLUM_WRITE_FAILED,
	NO_PAGE: E_VELLUM_NO_PAGE,
	DOCUMENT_ENDED: E_VELLUM_DOCUMENT_ENDED,
	INVALID_IMAGE: E_VELLUM_INVALID_IMAGE,
	INVALID_SVG: E_VELLUM_INVALID_SVG,
	INVALID_STRUCTURE: E_VELLUM_INVALID_STRUCTURE,
	INVALID_LINK: E_VELLUM_INVALID_LINK,
	INVALID_OUTLINE: E_VELLUM_INVALID_OUTLINE,
	INVALID_FONT: E_VELLUM_INVALID_FONT,
	NO_FONT: E_VELLUM_NO_FONT,
	PAGE_OUT_OF_RANGE: E_VELLUM_PAGE_OUT_OF_RANGE,
	UNBALANCED_RESTORE: E_VELLUM_UNBALANCED_RESTORE,
	NOTHING_TO_RESUME: E_VELLUM_NOTHING_TO_RESUME,
	VALIDATION: E_VELLUM_VALIDATION,
	DOCUMENT_BROKEN: E_VELLUM_DOCUMENT_BROKEN,
	INTERNAL: E_VELLUM_INTERNAL,
} as const satisfies Record<string, ErrorClass<[string]>>;

function isComposeCode(code: string): code is keyof typeof COMPOSE_ERRORS {
	return Object.hasOwn(COMPOSE_ERRORS, code);
}

/** Run a call on a document being authored, with its failures coded. */
export function compose<T>(work: () => T): T {
	try {
		return work();
	} catch (error) {
		throw composeFailure(error);
	}
}

export function createDocumentNative(
	settings: DocumentSettingsInput,
): NativeDocument {
	const loaded = engine();
	return compose(() => new loaded.NativeDocument(settings));
}

export function standardFontFileNative(name: string): string | null {
	return engine().standardFontFile(name);
}
