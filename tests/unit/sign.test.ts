import { createHash } from "node:crypto";
import { describe, expect, it } from "vitest";
import {
	A4,
	createBlank,
	type Signer,
	Vellum,
	VellumError,
} from "../../src/index.js";

/**
 * A signer that records what it was asked to sign. It is not a real one — the
 * point of these tests is the contract, not the cryptography: what the service
 * hands a signer, and what it does with what comes back.
 */
function recordingSigner(): Signer & { seen: Buffer[]; value: Buffer } {
	const value = Buffer.alloc(200, 0xab);
	const seen: Buffer[] = [];
	return {
		seen,
		value,
		async sign(digest: Buffer) {
			seen.push(digest);
			return value;
		},
	};
}

describe("signing", () => {
	it("hands the signer a digest and nothing else", async () => {
		const signer = recordingSigner();
		const vellum = new Vellum({ signers: { internal: signer } });

		await vellum.sign(createBlank([A4]), { signer: "internal" });

		expect(signer.seen).toHaveLength(1);
		// SHA-256, which is what the signer has to be prepared for.
		expect(signer.seen[0]).toHaveLength(32);
	});

	it("signs the document it gives back", async () => {
		const signer = recordingSigner();
		const vellum = new Vellum({ signers: { internal: signer } });
		const source = createBlank([A4]);

		const signed = await vellum.sign(source, { signer: "internal" });

		// The digest the signer was given has to be the digest of the byte
		// range the finished document declares — otherwise the signature
		// verifies against something that is not this file.
		const range = /\/ByteRange\s*\[([^\]]*)\]/.exec(signed.toString("latin1"));
		expect(range).not.toBeNull();
		const numbers = (range?.[1] ?? "").trim().split(/\s+/).map(Number);
		expect(numbers).toHaveLength(4);
		const [start = 0, first = 0, second = 0, length = 0] = numbers;

		const digest = createHash("sha256")
			.update(signed.subarray(start, start + first))
			.update(signed.subarray(second, second + length))
			.digest();
		expect(digest).toEqual(signer.seen[0]);
	});

	it("appends to the document rather than rewriting it", async () => {
		const vellum = new Vellum({ signers: { internal: recordingSigner() } });
		const source = createBlank([A4]);

		const signed = await vellum.sign(source, { signer: "internal" });

		// Rewriting would invalidate any signature already on the document.
		expect(signed.subarray(0, source.length)).toEqual(source);
		expect(signed.length).toBeGreaterThan(source.length);
	});

	it("leaves a document a reader can still open", async () => {
		const vellum = new Vellum({ signers: { internal: recordingSigner() } });
		const signed = await vellum.sign(createBlank([A4, A4]), {
			signer: "internal",
			reason: "Mandat de prévoyance",
			name: "Amélie Durand",
		});

		await expect(vellum.pageCount(signed)).resolves.toBe(2);
		const fields = await vellum.formFields(signed);
		expect(fields.map((field) => field.kind)).toContain("signature");
	});

	it("says which signers exist when the name is not one of them", async () => {
		const vellum = new Vellum({ signers: { internal: recordingSigner() } });

		await expect(
			vellum.sign(createBlank([A4]), { signer: "qualified" }),
		).rejects.toThrow(/declares internal/);
	});

	it("says so when nothing is configured to sign with", async () => {
		const vellum = new Vellum();

		await expect(
			vellum.sign(createBlank([A4]), { signer: "internal" }),
		).rejects.toThrow(VellumError);
	});

	it("refuses a signature larger than the space reserved", async () => {
		const vellum = new Vellum({
			signers: {
				oversized: {
					async sign() {
						return Buffer.alloc(600);
					},
				},
			},
		});

		await expect(
			vellum.sign(createBlank([A4]), { signer: "oversized", capacity: 512 }),
		).rejects.toThrow(/larger capacity/);
	});
});
