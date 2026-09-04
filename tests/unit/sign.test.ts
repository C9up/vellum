import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, expect, it, vi } from "vitest";
import {
	A4,
	createBlank,
	pkcs8Signer,
	type Signer,
	timestamped,
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

describe("signing with a key the application holds", () => {
	// The cryptography is proven in the engine's own tests, where a throwaway
	// key and certificate can be built and the result verified — including by
	// OpenSSL. What is worth testing here is the adapter: that it refuses
	// nonsense early, and that an engine refusal reaches the caller as one.
	it("refuses an empty key or certificate before anything else happens", () => {
		expect(() =>
			pkcs8Signer({ key: Buffer.alloc(0), certificate: Buffer.from([1]) }),
		).toThrow(VellumError);
		expect(() =>
			pkcs8Signer({ key: Buffer.from([1]), certificate: Buffer.alloc(0) }),
		).toThrow(VellumError);
	});

	it("says what it could not read rather than producing a broken document", async () => {
		const vellum = new Vellum({
			signers: {
				internal: pkcs8Signer({
					key: Buffer.from("this is not a key"),
					certificate: Buffer.from("nor is this a certificate"),
				}),
			},
		});

		await expect(
			vellum.sign(createBlank([A4]), { signer: "internal" }),
		).rejects.toThrow(/private key/);
	});
});

describe("timestamping", () => {
	/**
	 * A real CMS, because a timestamp query is built from one. It holds a
	 * certificate and a signature and no private key — see the engine test
	 * that writes it.
	 */
	function stubSigner(): Signer {
		const cms = readFileSync(
			fileURLToPath(new URL("../fixtures/signature.der", import.meta.url)),
		);
		return {
			async sign() {
				return cms;
			},
		};
	}

	it("posts the query the authority expects", async () => {
		let seen: { url: string; type: string | null; body: number } | undefined;
		const fetchSpy = async (url: string | URL, init?: RequestInit) => {
			const headers = new Headers(init?.headers);
			const body = init?.body;
			seen = {
				url: String(url),
				type: headers.get("content-type"),
				body: body instanceof Uint8Array ? body.byteLength : 0,
			};
			// Not a timestamp; the point here is what went out.
			return new Response(new Uint8Array([0x30, 0x00]), { status: 200 });
		};
		vi.stubGlobal("fetch", fetchSpy);

		const signer = timestamped(stubSigner(), { url: "https://tsa.test/tsr" });
		// The answer is nonsense, so attaching fails — after the request.
		await expect(signer.sign(Buffer.alloc(32))).rejects.toThrow(VellumError);

		expect(seen?.url).toBe("https://tsa.test/tsr");
		expect(seen?.type).toBe("application/timestamp-query");
		expect(seen?.body).toBeGreaterThan(0);
		vi.unstubAllGlobals();
	});

	it("says so when the authority cannot be reached", async () => {
		vi.stubGlobal("fetch", async () => {
			throw new Error("ECONNREFUSED");
		});

		const signer = timestamped(stubSigner(), { url: "https://tsa.test/tsr" });
		await expect(signer.sign(Buffer.alloc(32))).rejects.toThrow(
			/could not be reached/,
		);
		vi.unstubAllGlobals();
	});

	it("says so when the authority answers with an error", async () => {
		vi.stubGlobal("fetch", async () => new Response("no", { status: 503 }));

		const signer = timestamped(stubSigner(), { url: "https://tsa.test/tsr" });
		await expect(signer.sign(Buffer.alloc(32))).rejects.toThrow(/answered 503/);
		vi.unstubAllGlobals();
	});
});

/**
 * A signer returning the checked-in CMS. Its signature is real but commits to
 * a digest that is not the document's — which is the shape a tampered file
 * has, and lets the checks be exercised without a key.
 */
function stubCmsSigner(): Signer {
	const cms = readFileSync(
		fileURLToPath(new URL("../fixtures/signature.der", import.meta.url)),
	);
	return {
		async sign() {
			return cms;
		},
	};
}

describe("checking the signatures on a document", () => {
	it("reports none for a document that has none", async () => {
		const vellum = new Vellum();
		await expect(vellum.verifySignatures(createBlank([A4]))).resolves.toEqual(
			[],
		);
	});

	it("reports what it could not check rather than throwing", async () => {
		// The stub signer's CMS is real but signs a digest that is not this
		// document's, which is exactly the shape of a tampered file.
		const vellum = new Vellum({ signers: { internal: stubCmsSigner() } });
		const signed = await vellum.sign(createBlank([A4]), { signer: "internal" });

		const [report] = await vellum.verifySignatures(signed);
		expect(report?.coversWholeDocument).toBe(true);
		expect(report?.digestMatches).toBe(false);
		expect(report?.problems.join(" ")).toMatch(/changed since it was signed/);
	});

	it("catches content appended after the signature", async () => {
		const vellum = new Vellum({ signers: { internal: stubCmsSigner() } });
		const signed = await vellum.sign(createBlank([A4]), { signer: "internal" });
		const appended = Buffer.concat([signed, Buffer.from("\n% added later\n")]);

		const [report] = await vellum.verifySignatures(appended);
		expect(report?.coversWholeDocument).toBe(false);
		expect(report?.problems.join(" ")).toMatch(/added after this signature/);
	});

	it("refuses bytes that are not a PDF", async () => {
		const vellum = new Vellum();
		await expect(
			vellum.verifySignatures(Buffer.from("not a PDF")),
		).rejects.toThrow(VellumError);
	});
});
