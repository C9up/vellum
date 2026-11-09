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
import { createServer, type Server, type ServerResponse } from "node:http";
import type { AddressInfo } from "node:net";
import { fileURLToPath } from "node:url";
import { fakeHttp } from "@c9up/ream/http-client";
import { afterEach, describe, expect, it } from "vitest";
import { A4, createBlank } from "../src/index.js";
import {
	askResponder,
	mayAsk,
	type ResponderPermit,
} from "../src/responder.js";
import { Vellum } from "../src/Vellum.js";

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

/** A local server counting what it was asked, answering with `respond`. */
async function serve(
	respond: (response: ServerResponse) => void,
): Promise<{ origin: string; asked: string[]; server: Server }> {
	const asked: string[] = [];
	const server = createServer((request, response) => {
		asked.push(`${request.method} ${request.url}`);
		respond(response);
	});
	await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
	const address: AddressInfo | string | null = server.address();
	const port =
		typeof address === "object" && address !== null ? address.port : 0;
	return { origin: `http://127.0.0.1:${port}`, asked, server };
}

const servers: Server[] = [];
afterEach(async () => {
	for (const server of servers.splice(0)) {
		await new Promise<void>((resolve) => server.close(() => resolve()));
	}
});

const QUERY = new Uint8Array([0x30, 0x00]);

async function ask(url: string, named = false) {
	const target: ResponderPermit = { url: new URL(url), named };
	return askResponder(target, QUERY, { timeoutMs: 5_000 });
}

describe("vellum > the address a responder is asked at", () => {
	it("refuses private and special addresses written in the URL", async () => {
		for (const url of [
			"http://127.0.0.1:6379/",
			"http://10.0.0.5/",
			"http://169.254.169.254/latest/meta-data/",
			"http://100.64.0.1/",
			"http://[::1]:11211/",
			"http://[::ffff:127.0.0.1]/",
			"http://[64:ff9b::a9fe:a9fe]/", // NAT64 of the metadata address
			"http://[fec0::1]/", // site-local
			"http://[fd00::1]/",
		]) {
			const answer = await ask(url);
			expect(Buffer.isBuffer(answer), url).toBe(false);
			if (Buffer.isBuffer(answer)) continue;
			expect(answer.status, url).toBe("unknown");
			expect(answer.detail, url).toContain("an address on this network");
		}
	});

	it("refuses a name that resolves to this machine, at connect time", async () => {
		// `localhost` spells no address; DNS answers loopback. The spelling
		// check this replaced let every such name through.
		const local = await serve((response) => response.end("secret"));
		servers.push(local.server);
		const port = new URL(local.origin).port;
		const answer = await ask(`http://localhost:${port}/`);
		expect(Buffer.isBuffer(answer) ? "answered" : answer.detail).toContain(
			"an address on this network",
		);
		expect(local.asked).toEqual([]);
	});

	it("does not follow a redirect, even from a responder the application named", async () => {
		const inner = await serve((response) => response.end("secret"));
		const outer = await serve((response) => {
			response.writeHead(302, {
				location: `${inner.origin}/latest/meta-data/`,
			});
			response.end();
		});
		servers.push(inner.server, outer.server);
		const answer = await ask(`${outer.origin}/`, true);
		expect(Buffer.isBuffer(answer) ? "answered" : answer.detail).toContain(
			"redirected",
		);
		expect(outer.asked).toEqual(["POST /"]);
		expect(inner.asked).toEqual([]);
	});

	it("stops reading an answer larger than any OCSP answer", async () => {
		const flood = await serve((response) => {
			response.writeHead(200, { "content-type": "application/ocsp-response" });
			const chunk = Buffer.alloc(1024 * 1024);
			for (let sent = 0; sent < 10; sent++) response.write(chunk);
			response.end();
		});
		servers.push(flood.server);
		const answer = await ask(`${flood.origin}/`, true);
		expect(Buffer.isBuffer(answer) ? "answered" : answer.detail).toContain(
			"more than 64 KiB",
		);
	});

	it("asks a responder the application named on its own network", async () => {
		const local = await serve((response) =>
			response.end(Buffer.from([0x30, 0x00])),
		);
		servers.push(local.server);
		const answer = await ask(`${local.origin}/`, true);
		expect(Buffer.isBuffer(answer) ? [...answer] : answer).toEqual([
			0x30, 0x00,
		]);
		expect(local.asked).toEqual(["POST /"]);
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
		const http = fakeHttp();
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
