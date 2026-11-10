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
import {
	askResponder,
	type ClientModule,
	type GuardedClientOptions,
	MAX_RESPONDER_ANSWER,
	mayAsk,
	type ResponderClient,
	type ResponderPermit,
	readClientModule,
} from "../src/responder.js";
import { Vellum } from "../src/Vellum.js";
import { ClientFailure, fakeResponder } from "./fakeResponder.js";

const refusal = async (url: string, policy?: Parameters<typeof mayAsk>[1]) => {
	const answer = await mayAsk(url, policy);
	return "refused" in answer ? answer.refused : undefined;
};

const permit = async (url: string, policy?: Parameters<typeof mayAsk>[1]) => {
	const answer = await mayAsk(url, policy);
	return "refused" in answer ? undefined : answer;
};

describe("vellum > which responder may be named", () => {
	it("admits any http or https responder when no policy is set", async () => {
		expect((await permit("http://ocsp.example-ca.test/"))?.named).toBe(false);
		expect((await permit("https://ocsp.example-ca.test/x"))?.named).toBe(false);
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
			(await permit("http://ocsp.internal", ["ocsp.internal"]))?.named,
		).toBe(true);
		expect(
			await refusal("http://169.254.169.254/", ["ocsp.internal"]),
		).toContain("not among the responders");
	});

	it("does not open every port of a host it names", async () => {
		// Naming the authority used to admit any scheme and port on it.
		expect(
			await refusal("http://ocsp.internal:6379/", ["ocsp.internal"]),
		).toBeDefined();
		expect(
			await refusal("gopher://ocsp.internal/", ["ocsp.internal"]),
		).toBeDefined();
		expect(
			await permit("https://ocsp.internal/", ["ocsp.internal"]),
		).toBeDefined();
	});

	it("matches an origin on scheme, host and port", async () => {
		const origin = ["https://ocsp.internal:8443"];
		expect(
			await permit("https://ocsp.internal:8443/check", origin),
		).toBeDefined();
		expect(await refusal("http://ocsp.internal:8443/", origin)).toBeDefined();
		expect(await refusal("https://ocsp.internal:8444/", origin)).toBeDefined();
		expect(await refusal("https://ocsp.internal/", origin)).toBeDefined();
	});

	it("takes a predicate for an application that needs one", async () => {
		const onlyOurCa = (url: URL) => url.hostname.endsWith(".our-ca.test");
		expect((await permit("https://a.our-ca.test/", onlyOurCa))?.named).toBe(
			true,
		);
		expect(await refusal("https://evil.test/", onlyOurCa)).toContain("refused");
	});
});

const QUERY = new Uint8Array([0x30, 0x00]);
const RESPONDER = "http://ocsp.example-ca.test/";

async function ask(client: ResponderClient, url = RESPONDER, named = false) {
	const target: ResponderPermit = { url: new URL(url), named };
	return askResponder(target, QUERY, { timeoutMs: 5_000, client });
}

/** The detail of an answer that is not the responder's bytes. */
function detailOf(answer: Buffer | { detail?: string }): string | undefined {
	return Buffer.isBuffer(answer) ? undefined : answer.detail;
}

describe("vellum > asking a responder", () => {
	it("posts the query as an OCSP request, bounded in time and size", async () => {
		const http = fakeResponder().on(RESPONDER, { body: QUERY });
		const answer = await ask(http);

		expect(Buffer.isBuffer(answer) ? [...answer] : answer).toEqual([
			0x30, 0x00,
		]);
		expect(http.requests).toEqual([
			{
				url: RESPONDER,
				headers: { "content-type": "application/ocsp-request" },
				body: QUERY,
				timeout: 5_000,
				maxResponseBytes: MAX_RESPONDER_ANSWER,
			},
		]);
	});

	it("explains a refused private address, and how to allow it", async () => {
		const http = fakeResponder().on(
			RESPONDER,
			new ClientFailure("privateAddress", "connects to 10.0.0.5"),
		);
		const detail = detailOf(await ask(http));
		expect(detail).toContain("an address on this network");
		expect(detail).toContain("connects to 10.0.0.5");
		expect(detail).toContain("allowedResponders");
	});

	it("does not take a redirect for an answer", async () => {
		expect(
			detailOf(await ask(fakeResponder().on(RESPONDER, { status: 302 }))),
		).toContain("redirected");
		// A client the application gave may follow redirects itself, and
		// refuse one by its own rules.
		const refusing = fakeResponder().on(
			RESPONDER,
			new ClientFailure("redirect", "to another host"),
		);
		expect(detailOf(await ask(refusing))).toContain("redirected");
	});

	it("says so when the answer is larger than any OCSP answer", async () => {
		const http = fakeResponder().on(RESPONDER, {
			body: new Uint8Array(MAX_RESPONDER_ANSWER + 1),
		});
		expect(detailOf(await ask(http))).toContain("more than 64 KiB");
	});

	it("reports the status of an answer that is not a success", async () => {
		const http = fakeResponder().on(RESPONDER, { status: 503 });
		expect(detailOf(await ask(http))).toContain("answered 503");
	});

	it("reports a responder that could not be reached, whatever was thrown", async () => {
		expect(detailOf(await ask(fakeResponder()))).toContain(
			"could not be reached: nothing answers",
		);
		const throwing: ResponderClient = {
			async post() {
				throw "socket hang up";
			},
		};
		expect(detailOf(await ask(throwing))).toContain(
			"could not be reached: socket hang up",
		);
	});
});

/** An import failure as Node reports it. */
function importFailure(code: string): Error {
	const error = new Error(`import failed: ${code}`);
	Object.defineProperty(error, "code", { value: code });
	return error;
}

/** A client module whose clients answer `body`, recording how they were built. */
function recordingModule(body: Uint8Array | Error = QUERY) {
	const built: GuardedClientOptions[] = [];
	const closed: GuardedClientOptions[] = [];
	class HttpClient {
		readonly #options: GuardedClientOptions;
		constructor(options: GuardedClientOptions) {
			this.#options = options;
			built.push(options);
		}
		async post() {
			if (body instanceof Error) throw body;
			return { status: 200, body: Buffer.from(body) };
		}
		close() {
			closed.push(this.#options);
		}
	}
	const module: ClientModule = { HttpClient };
	return { module, built, closed };
}

describe("vellum > finding ream's client", () => {
	it("takes a missing module, or one without the subpath, as no client", async () => {
		for (const code of [
			"ERR_MODULE_NOT_FOUND",
			"ERR_PACKAGE_PATH_NOT_EXPORTED",
		]) {
			await expect(
				readClientModule(() => Promise.reject(importFailure(code))),
				code,
			).resolves.toBeUndefined();
		}
	});

	it("lets any other failure through", async () => {
		const broken = new SyntaxError("Unexpected token");
		await expect(readClientModule(() => Promise.reject(broken))).rejects.toBe(
			broken,
		);
		await expect(
			readClientModule(() =>
				Promise.reject(importFailure("ERR_UNKNOWN_FILE_EXTENSION")),
			),
		).rejects.toThrow("ERR_UNKNOWN_FILE_EXTENSION");
	});

	it("takes a module that does not carry a usable client as no client", async () => {
		class WithoutClose {
			async post() {
				return { status: 200, body: Buffer.alloc(0) };
			}
		}
		for (const loaded of [
			undefined,
			"@c9up/ream/http-client",
			{},
			{ HttpClient: "HttpClient" },
			{ HttpClient: () => undefined },
			{ HttpClient: WithoutClose },
		]) {
			await expect(
				readClientModule(async () => loaded),
				String(loaded),
			).resolves.toBeUndefined();
		}
	});

	it("hands back a module whose client can post and close", async () => {
		const { module } = recordingModule();
		await expect(readClientModule(async () => module)).resolves.toBe(module);
	});
});

describe("vellum > asking without a client of the application's", () => {
	it("refuses rather than ask through a client that does not guard", async () => {
		const answer = await askResponder(
			{ url: new URL(RESPONDER), named: false },
			QUERY,
			{ timeoutMs: 5_000, clientModule: async () => undefined },
		);
		expect(answer).toEqual({
			status: "unknown",
			detail: expect.stringContaining(
				"needs an outbound client that refuses private addresses",
			),
		});
	});

	it("builds ream's client following no redirect and exempting nothing", async () => {
		const { module, built, closed } = recordingModule();
		const answer = await askResponder(
			{ url: new URL(RESPONDER), named: false },
			QUERY,
			{ timeoutMs: 5_000, clientModule: async () => module },
		);
		expect(Buffer.isBuffer(answer)).toBe(true);
		expect(built).toEqual([{ maxRedirects: 0, allowHosts: [] }]);
		expect(closed).toEqual(built);
	});

	it("exempts exactly the host and port of a responder the application named", async () => {
		const { module, built } = recordingModule();
		for (const url of [
			"https://ocsp.internal/",
			"http://ocsp.internal/",
			"http://ocsp.internal:8080/",
		]) {
			await askResponder({ url: new URL(url), named: true }, QUERY, {
				timeoutMs: 5_000,
				clientModule: async () => module,
			});
		}
		expect(built.map((options) => options.allowHosts)).toEqual([
			["ocsp.internal:443"],
			["ocsp.internal:80"],
			["ocsp.internal:8080"],
		]);
	});

	it("closes the client it built even when asking fails", async () => {
		const { module, built, closed } = recordingModule(
			new ClientFailure("privateAddress", "connects to 127.0.0.1"),
		);
		const answer = await askResponder(
			{ url: new URL(RESPONDER), named: false },
			QUERY,
			{ timeoutMs: 5_000, clientModule: async () => module },
		);
		expect(detailOf(answer)).toContain("an address on this network");
		expect(closed).toEqual(built);
		expect(closed).toHaveLength(1);
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
		// through `verifySignatures` and watches the client rather than calling
		// the check again.
		const http = fakeResponder();
		const authority = readFileSync(
			fileURLToPath(new URL("./fixtures/authority.der", import.meta.url)),
		);
		const vellum = new Vellum({
			signers: { internal: stubSigner() },
			trustedAnchors: [authority],
			allowedResponders: [],
			httpClient: http,
		});
		const signed = await vellum.sign(createBlank([A4]), { signer: "internal" });

		const [report] = await vellum.verifySignatures(signed, {
			checkRevocation: true,
		});

		// The anchor is what gives the report an issuer, which is what takes
		// `#revocationOf` past its early return and up to the network.
		expect(report?.issuerCertificate).toBeDefined();
		expect(http.requests).toEqual([]);
		expect(report?.revocation?.status).toBe("unknown");
		expect(report?.revocation?.detail).toContain("not among the responders");
	});
});
