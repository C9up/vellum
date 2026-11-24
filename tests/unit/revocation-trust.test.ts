import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, expect, it, vi } from "vitest";
import { fakeResponder } from "../fakeResponder.js";

const authority = readFileSync(
	fileURLToPath(new URL("../fixtures/authority.der", import.meta.url)),
);

// The engine's report, as it comes back for a signature whose path to an
// anchor did not hold but whose issuer was found: the case the revocation
// question must not be asked in.
vi.mock("../../src/native.js", async (importOriginal) => {
	const actual = await importOriginal<typeof import("../../src/native.js")>();
	return {
		...actual,
		verifySignaturesNative: vi.fn(async () => [
			{
				field: "Signature1",
				coversWholeDocument: true,
				digestMatches: true,
				signatureVerifies: true,
				timestamped: false,
				trusted: false,
				chain: ["CN=Mallory", "CN=Mallory", "CN=Vellum Test Authority"],
				moment: "unknown",
				momentAt: 1_790_000_000,
				signerCertificate: authority,
				issuerCertificate: authority,
				problems: [
					"CN=Mallory signed a certificate without being an authority",
				],
			},
		]),
	};
});

const { Vellum } = await import("../../src/index.js");

describe("asking about revocation off a trusted path", () => {
	it("asks nobody, and answers unknown", async () => {
		const http = fakeResponder();
		const vellum = new Vellum({
			trustedAnchors: [authority],
			httpClient: http,
		});

		const [report] = await vellum.verifySignatures(Buffer.from("%PDF-1.7"), {
			checkRevocation: true,
		});

		expect(http.requests).toEqual([]);
		expect(report?.revocation?.status).toBe("unknown");
		expect(report?.revocation?.detail).toMatch(/no trusted path/);
	});
});
