/**
 * Where a document's fonts (and later its images) come from: the standard
 * fonts shipped in the package, files registered in memory, or the disk.
 */

import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { VellumError } from "../errors.js";

/** A file named by path, or its bytes. */
export type FileSource = string | URL | Uint8Array | ArrayBuffer;

const registered = new Map<string, Buffer>();

function key(path: string | URL): string {
	if (path instanceof URL) return fileURLToPath(path);
	return path.startsWith("file:") ? fileURLToPath(path) : path;
}

/**
 * Serve `path` from memory to every document that asks for it, before the
 * disk is looked at. `undefined` forgets it.
 */
export function registerFile(
	path: string | URL,
	data: Uint8Array | undefined,
): void {
	if (data === undefined) {
		registered.delete(key(path));
		return;
	}
	registered.set(key(path), Buffer.from(data));
}

const DATA_URL = /^data:[^,;]*(?:;[^,;]*)*;base64,/;

/** The bytes of a source: bytes, a `data:` URL in base64, or a path. */
export function readSource(source: FileSource): Buffer {
	if (source instanceof Uint8Array) return Buffer.from(source);
	if (source instanceof ArrayBuffer) return Buffer.from(new Uint8Array(source));
	if (typeof source === "string") {
		const header = DATA_URL.exec(source)?.[0];
		if (header !== undefined) {
			return Buffer.from(source.slice(header.length), "base64");
		}
	}
	const path = key(source);
	const fromMemory = registered.get(path);
	if (fromMemory !== undefined) return fromMemory;
	try {
		return readFileSync(path);
	} catch (error) {
		throw new VellumError(
			"FILE_NOT_FOUND",
			`cannot read ${JSON.stringify(path)}: ${error instanceof Error ? error.message : String(error)}`,
			{ cause: error },
		);
	}
}

const standardFonts = new Map<string, Buffer>();

/** A standard font's stand-in, read from the package once per process. */
export function standardFontBytes(file: string): Buffer {
	const cached = standardFonts.get(file);
	if (cached !== undefined) return cached;
	const bytes = readFileSync(new URL(`../../fonts/${file}`, import.meta.url));
	standardFonts.set(file, bytes);
	return bytes;
}
