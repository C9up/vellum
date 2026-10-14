/**
 * Where a document's fonts (and later its images) come from: the standard
 * fonts shipped in the package, files registered in memory, or the disk.
 */

import { readFileSync, statSync } from "node:fs";
import { basename } from "node:path";
import { fileURLToPath } from "node:url";
import { VellumError } from "../errors.js";

/** A file named by path, or its bytes. */
export type FileSource = string | URL | Uint8Array | ArrayBuffer;

/** A file's dates, as a file attachment records them. */
export interface FileStats {
	birthtime?: Date;
	ctime?: Date;
}

const registered = new Map<string, { data: Buffer; stats: FileStats }>();

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
	stats: FileStats = {},
): void {
	if (data === undefined) {
		registered.delete(key(path));
		return;
	}
	registered.set(key(path), { data: Buffer.from(data), stats });
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
	if (fromMemory !== undefined) return fromMemory.data;
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

/** What a file attachment records of its source: name, type and dates. */
export function describeSource(source: FileSource): {
	name: string | undefined;
	type: string | undefined;
	stats: FileStats;
} {
	if (typeof source === "string") {
		const media = /^data:([^,;]*)(?:;[^,;]*)*;base64,/.exec(source);
		if (media !== null) {
			return { name: undefined, type: media[1] || undefined, stats: {} };
		}
	}
	if (typeof source !== "string" && !(source instanceof URL)) {
		return { name: undefined, type: undefined, stats: {} };
	}
	const path = key(source);
	const known = registered.get(path);
	if (known !== undefined)
		return { name: basename(path), type: undefined, stats: known.stats };
	try {
		const { birthtime, ctime } = statSync(path);
		return {
			name: basename(path),
			type: undefined,
			stats: { birthtime, ctime },
		};
	} catch {
		return { name: basename(path), type: undefined, stats: {} };
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
