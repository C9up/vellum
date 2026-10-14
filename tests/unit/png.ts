/**
 * Just enough PNG decoding to read a rendered page's pixels back: 8-bit
 * RGB or RGBA, non-interlaced, as the engine writes them.
 */

import { deflateSync, inflateSync } from "node:zlib";

export interface Pixels {
	width: number;
	height: number;
	at(x: number, y: number): [number, number, number];
}

function paeth(left: number, up: number, upLeft: number): number {
	const estimate = left + up - upLeft;
	const toLeft = Math.abs(estimate - left);
	const toUp = Math.abs(estimate - up);
	const toUpLeft = Math.abs(estimate - upLeft);
	if (toLeft <= toUp && toLeft <= toUpLeft) return left;
	return toUp <= toUpLeft ? up : upLeft;
}

export function decodePng(png: Buffer): Pixels {
	let offset = 8;
	let width = 0;
	let height = 0;
	let channels = 0;
	const data: Buffer[] = [];
	while (offset < png.length) {
		const length = png.readUInt32BE(offset);
		const type = png.toString("latin1", offset + 4, offset + 8);
		const body = png.subarray(offset + 8, offset + 8 + length);
		if (type === "IHDR") {
			width = body.readUInt32BE(0);
			height = body.readUInt32BE(4);
			const colourType = body.readUInt8(9);
			if (body.readUInt8(8) !== 8 || body.readUInt8(12) !== 0) {
				throw new Error("only 8-bit, non-interlaced PNG");
			}
			channels = colourType === 6 ? 4 : colourType === 2 ? 3 : 0;
			if (channels === 0) throw new Error(`colour type ${colourType}`);
		} else if (type === "IDAT") {
			data.push(body);
		}
		offset += 12 + length;
	}
	const raw = inflateSync(Buffer.concat(data));
	const stride = width * channels;
	const pixels = Buffer.alloc(stride * height);
	for (let row = 0; row < height; row++) {
		const filter = raw.readUInt8(row * (stride + 1));
		for (let column = 0; column < stride; column++) {
			const value = raw.readUInt8(row * (stride + 1) + 1 + column);
			const left =
				column >= channels
					? pixels.readUInt8(row * stride + column - channels)
					: 0;
			const up = row > 0 ? pixels.readUInt8((row - 1) * stride + column) : 0;
			const upLeft =
				row > 0 && column >= channels
					? pixels.readUInt8((row - 1) * stride + column - channels)
					: 0;
			const predictor =
				[0, left, up, (left + up) >> 1, paeth(left, up, upLeft)][filter] ?? 0;
			pixels.writeUInt8((value + predictor) & 0xff, row * stride + column);
		}
	}
	return {
		width,
		height,
		at(x, y) {
			const index = (y * width + x) * channels;
			return [
				pixels.readUInt8(index),
				pixels.readUInt8(index + 1),
				pixels.readUInt8(index + 2),
			];
		},
	};
}

function crc32(bytes: Buffer): number {
	let crc = 0xffffffff;
	for (const byte of bytes) {
		crc ^= byte;
		for (let bit = 0; bit < 8; bit++) {
			crc = crc & 1 ? (crc >>> 1) ^ 0xedb88320 : crc >>> 1;
		}
	}
	return (crc ^ 0xffffffff) >>> 0;
}

function chunk(type: string, body: Buffer): Buffer {
	const length = Buffer.alloc(4);
	length.writeUInt32BE(body.length);
	const named = Buffer.concat([Buffer.from(type, "latin1"), body]);
	const crc = Buffer.alloc(4);
	crc.writeUInt32BE(crc32(named));
	return Buffer.concat([length, named, crc]);
}

/** An RGB PNG whose every pixel `colour(x, y)` decides. */
export function encodePng(
	width: number,
	height: number,
	colour: (x: number, y: number) => [number, number, number],
): Buffer {
	const header = Buffer.alloc(13);
	header.writeUInt32BE(width, 0);
	header.writeUInt32BE(height, 4);
	header.writeUInt8(8, 8);
	header.writeUInt8(2, 9);
	const raw = Buffer.alloc((width * 3 + 1) * height);
	for (let y = 0; y < height; y++) {
		for (let x = 0; x < width; x++) {
			const [r, g, b] = colour(x, y);
			const at = y * (width * 3 + 1) + 1 + x * 3;
			raw.writeUInt8(r, at);
			raw.writeUInt8(g, at + 1);
			raw.writeUInt8(b, at + 2);
		}
	}
	return Buffer.concat([
		Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
		chunk("IHDR", header),
		chunk("IDAT", deflateSync(raw)),
		chunk("IEND", Buffer.alloc(0)),
	]);
}
