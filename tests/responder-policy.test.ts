/**
 * Which revocation responder may be contacted.
 *
 * The address comes out of the certificate inside the document being checked —
 * from whoever sent it. Following it unconditionally turns "check whether this
 * signature is still valid" into "make my server issue an HTTP request wherever
 * this stranger points": the cloud metadata endpoint, a service on loopback, or
 * a URL whose only purpose is to report that the document was opened and the
 * address it was opened from.
 */

import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { A4, createBlank } from "../src/index.js";
import { mayAsk } from "../src/responder.js";
import { Vellum } from "../src/Vellum.js";

const refusal = async (url: string, policy?: Parameters<typeof mayAsk>[1]) => {
	const answer = await mayAsk(url, policy);
	return "refused" in answer ? answer.refused : undefined;
};

describe("vellum > which responder may be asked", () => {
	it("asks a public authority over http", async () => {
		expect(await mayAsk("http://ocsp.example-ca.test/")).toBeInstanceOf(URL);
		expect(await mayAsk("https://ocsp.example-ca.test/x")).toBeInstanceOf(URL);
	});

	it("refuses the cloud metadata address", async () => {
		// The one that hands out credentials to anyone who can make the server
		// ask for them.
		const why = await refusal(
			"http://169.254.169.254/latest/meta-data/iam/security-credentials/",
		);
		expect(why).toContain("169.254.169.254");
		expect(why).toContain("an address on this network");
	});

	it("refuses loopback and the private ranges", async () => {
		for (const url of [
			"http://127.0.0.1:6379/",
			"http://localhost:5432/",
			"http://10.0.0.5/",
			"http://172.16.4.1/",
			"http://192.168.1.1/",
			"http://100.64.0.1/", // carrier-grade NAT
			"http://0.0.0.0/",
		]) {
			expect(await refusal(url), url).toBeDefined();
		}
	});

	it("refuses the same addresses written as IPv6", async () => {
		// `::ffff:127.0.0.1` is loopback under another spelling, and a check that
		// only reads dotted quads waves it through.
		for (const url of [
			"http://[::1]:11211/",
			"http://[::ffff:127.0.0.1]/",
			"http://[fd00::1]/",
			"http://[fe80::1]/",
		]) {
			expect(await refusal(url), url).toBeDefined();
		}
	});

	it("refuses a scheme that is not how OCSP is asked", async () => {
		expect(await refusal("file:///etc/passwd")).toContain("not asked");
		expect(await refusal("gopher://x/")).toContain("not asked");
	});

	it("says so when the certificate names something that is not a URL", async () => {
		expect(await refusal("not a url")).toContain("not a URL");
	});

	it("lets an application name its own responder", async () => {
		// An internal authority is legitimate — but the application says so, not
		// the document.
		expect(
			await mayAsk("http://ocsp.internal", ["ocsp.internal"]),
		).toBeInstanceOf(URL);
		expect(
			await refusal("http://169.254.169.254/", ["ocsp.internal"]),
		).toContain("not among the responders");
	});

	it("takes a predicate for an application that needs one", async () => {
		const onlyOurCa = (url: URL) => url.hostname.endsWith(".our-ca.test");
		expect(await mayAsk("https://a.our-ca.test/", onlyOurCa)).toBeInstanceOf(
			URL,
		);
		expect(await refusal("https://evil.test/", onlyOurCa)).toContain("refused");
	});

	it("does not let a public-looking host smuggle a private one", async () => {
		// A hostname that RESOLVES to a private address still gets through: DNS
		// is not consulted here. Named so nobody reads more into this than it
		// does — `allowedResponders` is the airtight form.
		expect(await mayAsk("http://rebind.example.test/")).toBeInstanceOf(URL);
	});
});

describe("vellum > the guard sits between the certificate and the network", () => {
	/** A signer returning the checked-in CMS — real, and not this document's. */
	function stubSigner() {
		const cms = readFileSync(
			fileURLToPath(new URL("./fixtures/signature.der", import.meta.url)),
		);
		return {
			async sign() {
				return cms;
			},
		};
	}

	it("makes no request when the policy allows no responder", async () => {
		// A policy the code never consults protects nothing, so this goes
		// through `verifySignatures` and watches the socket rather than calling
		// the check again.
		const attempted: string[] = [];
		const realFetch = globalThis.fetch;
		globalThis.fetch = (async (input: Parameters<typeof fetch>[0]) => {
			attempted.push(String(input));
			throw new Error("no request should have been made");
		}) as typeof fetch;

		try {
			const authority = readFileSync(
				fileURLToPath(new URL("./fixtures/authority.der", import.meta.url)),
			);
			const vellum = new Vellum({
				signers: { internal: stubSigner() },
				trustedAnchors: [authority],
				allowedResponders: [],
			});
			const signed = await vellum.sign(createBlank([A4]), {
				signer: "internal",
			});

			const [report] = await vellum.verifySignatures(signed, {
				checkRevocation: true,
			});

			// The anchor is what gives the report an issuer, which is what takes
			// `#revocationOf` past its early return and up to the network.
			expect(report?.issuerCertificate).toBeDefined();
			expect(attempted).toEqual([]);
			expect(report?.revocation?.status).toBe("unknown");
			expect(report?.revocation?.detail).toBeTruthy();
		} finally {
			globalThis.fetch = realFetch;
		}
	});
});
