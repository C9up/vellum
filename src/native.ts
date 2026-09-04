/**
 * Loader for the Rust PDF engine.
 *
 * The engine is not optional. Parsing, authoring and rendering PDF have no
 * JavaScript implementation in this package, so a missing binary is a hard
 * failure with an actionable message — never a silent degradation that lets
 * one deployment behave differently from another.
 */

import { createRequire } from "node:module";
import { dirname, join } from "node:path";
import { arch, platform } from "node:process";
import { fileURLToPath } from "node:url";
import { VellumError } from "./errors.js";
import type {
	DocumentInfo,
	DocumentMetadata,
	FormField,
	SignatureOptions as NativeSignatureOptions,
	StampOptions as NativeStampOptions,
	TextStampOptions as NativeTextStampOptions,
	TrustOptions as NativeTrustOptions,
	PageDimensions,
	PageSize,
	PreparedSignature,
	RenderOptions,
	SignatureReport,
	TimestampQuery,
} from "./native/generated.js";

const requireNative = createRequire(import.meta.url);
const here = dirname(fileURLToPath(import.meta.url));

const platformMap: Record<string, string> = {
	"linux-x64": "linux-x64-gnu",
	"linux-arm64": "linux-arm64-gnu",
	"darwin-x64": "darwin-x64",
	"darwin-arm64": "darwin-arm64",
	"win32-x64": "win32-x64-msvc",
};

/**
 * The engine's surface, as the Rust declares it.
 *
 * Derived from `./native/generated.js` — written by `pnpm build:napi-types`
 * from napi-derive's own `type-def` output — rather than restated here, where
 * nothing would notice a `pub fn` gaining a parameter or changing its return.
 */
type NativeVellum = typeof import("./native/generated.js");

export type {
	DocumentInfo,
	DocumentMetadata,
	FormField,
	PageDimensions,
	PageSize,
	PreparedSignature,
	SignatureReport,
} from "./native/generated.js";

let native: NativeVellum | undefined;
let loadError: unknown;

try {
	const suffix = platformMap[`${platform}-${arch}`];
	if (suffix) {
		native = requireNative(join(here, `../index.${suffix}.node`));
	}
} catch (error) {
	loadError = error;
}

export function isNativeAvailable(): boolean {
	return native !== undefined;
}

/** Why the engine could not be loaded, phrased for whoever has to fix it. */
function unavailableReason(): string {
	const target = `${platform}-${arch}`;
	if (loadError !== undefined) {
		return `failed to load (${loadError instanceof Error ? loadError.message : String(loadError)})`;
	}
	return platformMap[target] !== undefined
		? "binary not found"
		: `no prebuilt binary for ${target}`;
}

/** Raised when an operation needs the Rust engine and it is not there. */
export class VellumNativeRequiredError extends VellumError {
	constructor() {
		super(
			"NAPI_REQUIRED",
			`The Rust PDF engine is required but not loaded — ${unavailableReason()}.\n` +
				"Install the prebuilt binary for this platform, or build it with `pnpm build:napi`.",
		);
		this.name = "VellumNativeRequiredError";
	}
}

/** The engine, or a refusal explaining how to get one. */
function engine(): NativeVellum {
	if (native === undefined) throw new VellumNativeRequiredError();
	return native;
}

/**
 * Run engine work, translating its failures into coded errors.
 *
 * The engine reports failures as plain reasons. Without this, a caller
 * catching a corrupt upload would get an `Error` it cannot branch on.
 */
function run<T>(code: string, work: (engine: NativeVellum) => T): T {
	const loaded = engine();
	try {
		return work(loaded);
	} catch (error) {
		throw new VellumError(
			code,
			error instanceof Error ? error.message : String(error),
			{ cause: error },
		);
	}
}

export function inspectNative(pdf: Buffer): DocumentInfo {
	return run("INVALID_PDF", (loaded) => loaded.inspect(pdf));
}

export function createBlankNative(pages: ReadonlyArray<PageSize>): Buffer {
	return run("WRITE_FAILED", (loaded) => loaded.createBlank([...pages]));
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
		throw new VellumError(
			"RENDER_FAILED",
			error instanceof Error ? error.message : String(error),
			{ cause: error },
		);
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
		throw new VellumError(
			"RENDER_FAILED",
			error instanceof Error ? error.message : String(error),
			{ cause: error },
		);
	}
}

export function pageDimensionsNative(pdf: Buffer): PageDimensions[] {
	return run("INVALID_PDF", (loaded) => loaded.pageDimensions(pdf));
}

export function metadataNative(pdf: Buffer): DocumentMetadata {
	return run("INVALID_PDF", (loaded) => loaded.metadata(pdf));
}

export async function extractTextNative(
	pdf: Buffer,
	pageIndex: number,
): Promise<string> {
	const loaded = engine();
	try {
		return await loaded.extractText(pdf, pageIndex);
	} catch (error) {
		throw new VellumError(
			"EXTRACT_FAILED",
			error instanceof Error ? error.message : String(error),
			{ cause: error },
		);
	}
}

export async function extractTextAllNative(pdf: Buffer): Promise<string[]> {
	const loaded = engine();
	try {
		return await loaded.extractTextAll(pdf);
	} catch (error) {
		throw new VellumError(
			"EXTRACT_FAILED",
			error instanceof Error ? error.message : String(error),
			{ cause: error },
		);
	}
}

async function edit<T>(
	code: string,
	work: (engine: NativeVellum) => Promise<T>,
): Promise<T> {
	const loaded = engine();
	try {
		return await work(loaded);
	} catch (error) {
		throw new VellumError(
			code,
			error instanceof Error ? error.message : String(error),
			{ cause: error },
		);
	}
}

export function mergeNative(documents: ReadonlyArray<Buffer>): Promise<Buffer> {
	return edit("MERGE_FAILED", (loaded) => loaded.merge([...documents]));
}

export function selectPagesNative(
	pdf: Buffer,
	pageIndexes: ReadonlyArray<number>,
): Promise<Buffer> {
	return edit("SELECT_FAILED", (loaded) =>
		loaded.selectPages(pdf, [...pageIndexes]),
	);
}

export function splitNative(pdf: Buffer): Promise<Buffer[]> {
	return edit("SPLIT_FAILED", (loaded) => loaded.split(pdf));
}

export function rotateNative(
	pdf: Buffer,
	degrees: number,
	pageIndexes?: ReadonlyArray<number>,
): Promise<Buffer> {
	return edit("ROTATE_FAILED", (loaded) =>
		loaded.rotate(pdf, degrees, pageIndexes ? [...pageIndexes] : undefined),
	);
}

export function stampNative(
	pdf: Buffer,
	image: Buffer,
	options: NativeStampOptions,
): Promise<Buffer> {
	return edit("STAMP_FAILED", (loaded) => loaded.stamp(pdf, image, options));
}

export function stampTextNative(
	pdf: Buffer,
	text: string,
	options: NativeTextStampOptions,
): Promise<Buffer> {
	return edit("STAMP_TEXT_FAILED", (loaded) =>
		loaded.stampText(pdf, text, options),
	);
}

export function formFieldsNative(pdf: Buffer): FormField[] {
	return run("INVALID_PDF", (loaded) => loaded.formFields(pdf));
}

export function fillFormNative(
	pdf: Buffer,
	values: Record<string, string>,
): Promise<Buffer> {
	return edit("FILL_FAILED", (loaded) => loaded.fillForm(pdf, values));
}

export function flattenFormNative(pdf: Buffer): Promise<Buffer> {
	return edit("FLATTEN_FAILED", (loaded) => loaded.flattenForm(pdf));
}

export function prepareSignatureNative(
	pdf: Buffer,
	options: NativeSignatureOptions,
): Promise<PreparedSignature> {
	return edit("SIGN_FAILED", (loaded) => loaded.prepareSignature(pdf, options));
}

export function embedSignatureNative(
	prepared: Buffer,
	value: Buffer,
): Promise<Buffer> {
	return edit("SIGN_FAILED", (loaded) =>
		loaded.embedSignature(prepared, value),
	);
}

export function signCmsNative(
	digest: Buffer,
	key: Buffer,
	certificates: ReadonlyArray<Buffer>,
	signedAt: string,
): Promise<Buffer> {
	return edit("SIGN_FAILED", (loaded) =>
		loaded.signCms(digest, key, [...certificates], signedAt),
	);
}

export function timestampQueryNative(cms: Buffer): TimestampQuery {
	return run("TIMESTAMP_FAILED", (loaded) => loaded.timestampQuery(cms));
}

export function attachTimestampNative(
	cms: Buffer,
	response: Buffer,
	nonce: Buffer,
): Buffer {
	return run("TIMESTAMP_FAILED", (loaded) =>
		loaded.attachTimestamp(cms, response, nonce),
	);
}

export function verifySignaturesNative(
	pdf: Buffer,
	trust: NativeTrustOptions,
): Promise<SignatureReport[]> {
	return edit("VERIFY_FAILED", (loaded) => loaded.verifySignatures(pdf, trust));
}
