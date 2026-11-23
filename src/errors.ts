/**
 * Vellum errors, one class per code. An application branches on
 * `error.code` or on `error instanceof errors.E_…`; every one is a flare
 * `Exception`, with the HTTP status an exception handler answers with.
 *
 * The engine names its document failures `CODE: reason`; each such code has
 * a class here, `E_VELLUM_` before it.
 */

import { createError } from "@c9up/flare";

/** The bytes are not a PDF vellum can read. */
export const E_VELLUM_INVALID_PDF = createError<[string]>(
	"%s",
	"E_VELLUM_INVALID_PDF",
	422,
);

/** An edit would invalidate the document's signatures. */
export const E_VELLUM_SIGNED_DOCUMENT = createError<[string]>(
	"%s",
	"E_VELLUM_SIGNED_DOCUMENT",
);

/** A page number is not a whole number from 1. */
export const E_VELLUM_INVALID_PAGE = createError<[string]>(
	"%s",
	"E_VELLUM_INVALID_PAGE",
);

/** Merging failed. */
export const E_VELLUM_MERGE_FAILED = createError<[string]>(
	"%s",
	"E_VELLUM_MERGE_FAILED",
);

/** Selecting pages failed. */
export const E_VELLUM_SELECT_FAILED = createError<[string]>(
	"%s",
	"E_VELLUM_SELECT_FAILED",
);

/** Splitting failed. */
export const E_VELLUM_SPLIT_FAILED = createError<[string]>(
	"%s",
	"E_VELLUM_SPLIT_FAILED",
);

/** Rotating failed. */
export const E_VELLUM_ROTATE_FAILED = createError<[string]>(
	"%s",
	"E_VELLUM_ROTATE_FAILED",
);

/** Stamping an image failed. */
export const E_VELLUM_STAMP_FAILED = createError<[string]>(
	"%s",
	"E_VELLUM_STAMP_FAILED",
);

/** Stamping text failed. */
export const E_VELLUM_STAMP_TEXT_FAILED = createError<[string]>(
	"%s",
	"E_VELLUM_STAMP_TEXT_FAILED",
);

/** Filling the form failed. */
export const E_VELLUM_FILL_FAILED = createError<[string]>(
	"%s",
	"E_VELLUM_FILL_FAILED",
);

/** Flattening the form failed. */
export const E_VELLUM_FLATTEN_FAILED = createError<[string]>(
	"%s",
	"E_VELLUM_FLATTEN_FAILED",
);

/** Rendering a page failed. */
export const E_VELLUM_RENDER_FAILED = createError<[string]>(
	"%s",
	"E_VELLUM_RENDER_FAILED",
);

/** Extracting text failed. */
export const E_VELLUM_EXTRACT_FAILED = createError<[string]>(
	"%s",
	"E_VELLUM_EXTRACT_FAILED",
);

/** Writing the document failed. */
export const E_VELLUM_WRITE_FAILED = createError<[string]>(
	"%s",
	"E_VELLUM_WRITE_FAILED",
);

/** Signing failed. */
export const E_VELLUM_SIGN_FAILED = createError<[string]>(
	"%s",
	"E_VELLUM_SIGN_FAILED",
);

/** Verifying the signatures failed. */
export const E_VELLUM_VERIFY_FAILED = createError<[string]>(
	"%s",
	"E_VELLUM_VERIFY_FAILED",
);

/** Building or reading a timestamp failed. */
export const E_VELLUM_TIMESTAMP_FAILED = createError<[string]>(
	"%s",
	"E_VELLUM_TIMESTAMP_FAILED",
);

/** No signer of this name is configured. */
export const E_VELLUM_UNKNOWN_SIGNER = createError<[string]>(
	"%s",
	"E_VELLUM_UNKNOWN_SIGNER",
);

/** A document, page, table, drawing option or ceiling is invalid. */
export const E_VELLUM_INVALID_OPTIONS = createError<[string]>(
	"%s",
	"E_VELLUM_INVALID_OPTIONS",
);

/** A colour cannot be read. */
export const E_VELLUM_INVALID_COLOR = createError<[string]>(
	"%s",
	"E_VELLUM_INVALID_COLOR",
);

/** An image cannot be used. */
export const E_VELLUM_INVALID_IMAGE = createError<[string]>(
	"%s",
	"E_VELLUM_INVALID_IMAGE",
);

/** An SVG cannot be used. */
export const E_VELLUM_INVALID_SVG = createError<[string]>(
	"%s",
	"E_VELLUM_INVALID_SVG",
);

/** A path holds an operator vellum does not draw. */
export const E_VELLUM_UNSUPPORTED_CONTENT = createError<[string]>(
	"%s",
	"E_VELLUM_UNSUPPORTED_CONTENT",
);

/** A structure element was changed after it ended. */
export const E_VELLUM_INVALID_STRUCTURE = createError<[string]>(
	"%s",
	"E_VELLUM_INVALID_STRUCTURE",
);

/** There is no current page. */
export const E_VELLUM_NO_PAGE = createError<[string]>("%s", "E_VELLUM_NO_PAGE");

/** The document has ended. */
export const E_VELLUM_DOCUMENT_ENDED = createError<[string]>(
	"%s",
	"E_VELLUM_DOCUMENT_ENDED",
);

/** A link or a named destination cannot be used. */
export const E_VELLUM_INVALID_LINK = createError<[string]>(
	"%s",
	"E_VELLUM_INVALID_LINK",
);

/** An outline entry cannot be used. */
export const E_VELLUM_INVALID_OUTLINE = createError<[string]>(
	"%s",
	"E_VELLUM_INVALID_OUTLINE",
);

/** A font cannot be used. */
export const E_VELLUM_INVALID_FONT = createError<[string]>(
	"%s",
	"E_VELLUM_INVALID_FONT",
);

/** Text was written before a font was selected. */
export const E_VELLUM_NO_FONT = createError<[string]>("%s", "E_VELLUM_NO_FONT");

/** A page index names no page of the document. */
export const E_VELLUM_PAGE_OUT_OF_RANGE = createError<[string]>(
	"%s",
	"E_VELLUM_PAGE_OUT_OF_RANGE",
);

/** `restore()` was called without a matching `save()`. */
export const E_VELLUM_UNBALANCED_RESTORE = createError<[string]>(
	"%s",
	"E_VELLUM_UNBALANCED_RESTORE",
);

/** No text is waiting for a page. */
export const E_VELLUM_NOTHING_TO_RESUME = createError<[string]>(
	"%s",
	"E_VELLUM_NOTHING_TO_RESUME",
);

/** The document breaks a rule of the PDF it declares (PDF/A, PDF/UA, a form or an annotation rule). */
export const E_VELLUM_VALIDATION = createError<[string]>(
	"%s",
	"E_VELLUM_VALIDATION",
);

/** An earlier call failed inside the engine; the document cannot be used further. */
export const E_VELLUM_DOCUMENT_BROKEN = createError<[string]>(
	"%s",
	"E_VELLUM_DOCUMENT_BROKEN",
);

/** The engine panicked. */
export const E_VELLUM_INTERNAL = createError<[string]>(
	"%s",
	"E_VELLUM_INTERNAL",
);

/** A rotation is not a multiple of 90. */
export const E_VELLUM_INVALID_ROTATION = createError<[number]>(
	"Rotation must be a whole multiple of 90, got %s.",
	"E_VELLUM_INVALID_ROTATION",
);

/** `merge()` was given no document. */
export const E_VELLUM_EMPTY_MERGE = createError(
	"Merging needs at least one document.",
	"E_VELLUM_EMPTY_MERGE",
);

/** `selectPages()` was given no page. */
export const E_VELLUM_EMPTY_SELECTION = createError(
	"Selecting needs at least one page.",
	"E_VELLUM_EMPTY_SELECTION",
);

/** A signer's key or certificate is empty. */
export const E_VELLUM_SIGNER_INVALID = createError<[string]>(
	"The %s is empty.",
	"E_VELLUM_SIGNER_INVALID",
);

/** The timestamp authority could not be reached. */
export const E_VELLUM_TIMESTAMP_UNREACHABLE = createError<[string]>(
	"The timestamp authority at %s could not be reached.",
	"E_VELLUM_TIMESTAMP_UNREACHABLE",
);

/** The timestamp authority answered with an error status. */
export const E_VELLUM_TIMESTAMP_REFUSED = createError<[string, number]>(
	"The timestamp authority at %s answered %s.",
	"E_VELLUM_TIMESTAMP_REFUSED",
);

/** A configured font file cannot be read. */
export const E_VELLUM_FONT_UNREADABLE = createError<[string, string]>(
	"The font %s is configured as %s, which cannot be read.",
	"E_VELLUM_FONT_UNREADABLE",
);

/** A file given by path cannot be read. */
export const E_VELLUM_FILE_NOT_FOUND = createError<[string, string]>(
	"cannot read %s: %s",
	"E_VELLUM_FILE_NOT_FOUND",
);

/** Form fields were added before `initForm()`. */
export const E_VELLUM_NO_FORM = createError(
	"call initForm() before adding form fields",
	"E_VELLUM_NO_FORM",
);

/** A row was added to a table after `lastRow` ended it. */
export const E_VELLUM_TABLE_ENDED = createError<[number]>(
	"Table was marked as ended on row %s",
	"E_VELLUM_TABLE_ENDED",
);

/** The Rust PDF engine is not loaded. */
export const E_VELLUM_NAPI_REQUIRED = createError<[string]>(
	"The Rust PDF engine is required but not loaded — %s.\n" +
		"Install the prebuilt binary for this platform, or build it with `pnpm build:napi`.",
	"E_VELLUM_NAPI_REQUIRED",
);

/** The `services/main` service was used before `VellumProvider` booted. */
export const E_VELLUM_NOT_BOOTED = createError(
	"[vellum] Vellum service accessed before VellumProvider.boot() ran. " +
		"Check that `@c9up/vellum/provider` is listed in your reamrc.ts providers.",
	"E_VELLUM_NOT_BOOTED",
);
