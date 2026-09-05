/**
 * Deciding whether a revocation responder may be contacted.
 *
 * The address comes out of the certificate inside the document being checked,
 * which is to say: from whoever sent the document. Following it unconditionally
 * turns "check whether this signature is still valid" into "make my server
 * issue an HTTP request wherever this stranger points" — the cloud metadata
 * endpoint, a Redis on loopback, or simply a URL that confirms the document was
 * opened and reveals the address it was opened from.
 *
 * So the default is a public responder over HTTP, and an application whose
 * authority answers on the internal network says so by name. Everything
 * refused answers `"unknown"`, never `"good"`: a responder that was not asked
 * has told us nothing.
 */

/** Who an application is willing to ask about revocation. */
export type ResponderPolicy =
	| ReadonlyArray<string>
	| ((url: URL) => boolean | Promise<boolean>);

/** Hosts that are never a public certificate authority. */
function isPrivateHost(hostname: string): boolean {
	const host = hostname.replace(/^\[|\]$/g, "").toLowerCase();
	if (host === "localhost" || host.endsWith(".localhost")) return true;
	// IPv6 loopback, unspecified, unique-local (fc00::/7) and link-local (fe80::/10).
	if (host === "::1" || host === "::") return true;
	if (/^f[cd][0-9a-f]{2}:/.test(host)) return true;
	if (/^fe[89ab][0-9a-f]:/.test(host)) return true;
	// An IPv4-mapped IPv6 address is the same machine under another spelling,
	// and the URL parser rewrites `::ffff:127.0.0.1` as `::ffff:7f00:1` — so a
	// check that only reads dotted quads waves loopback straight through.
	const mapped =
		/^::ffff:(\d+\.\d+\.\d+\.\d+)$/.exec(host) ??
		(() => {
			const hex = /^::ffff:([0-9a-f]{1,4}):([0-9a-f]{1,4})$/.exec(host);
			if (hex === null) return null;
			const packed =
				(Number.parseInt(hex[1] as string, 16) << 16) |
				Number.parseInt(hex[2] as string, 16);
			return [
				host,
				[24, 16, 8, 0].map((shift) => (packed >>> shift) & 0xff).join("."),
			];
		})();
	const literal = mapped?.[1] ?? host;
	const parts = literal.split(".");
	if (parts.length !== 4 || !parts.every((p) => /^\d{1,3}$/.test(p)))
		return false;
	const [a, b] = parts.map(Number) as [number, number, number, number];
	return (
		a === 0 || // this network
		a === 127 || // loopback
		a === 10 || // private
		(a === 172 && b >= 16 && b <= 31) || // private
		(a === 192 && b === 168) || // private
		(a === 169 && b === 254) || // link-local, and the cloud metadata address
		(a === 100 && b >= 64 && b <= 127) || // carrier-grade NAT
		a >= 224 // multicast and reserved
	);
}

/** Why a responder was refused, in a sentence a caller can act on. */
export interface ResponderRefusal {
	refused: string;
}

/**
 * Decide whether `url` may be asked.
 *
 * @param policy What the application allows: a list of hostnames, or a
 *   predicate. Absent, only public hosts over http/https are contacted.
 */
export async function mayAsk(
	url: string,
	policy?: ResponderPolicy,
): Promise<URL | ResponderRefusal> {
	let parsed: URL;
	try {
		parsed = new URL(url);
	} catch {
		return { refused: `the certificate names "${url}", which is not a URL` };
	}

	if (typeof policy === "function") {
		return (await policy(parsed))
			? parsed
			: { refused: `the configured policy refused ${parsed.origin}` };
	}
	if (policy !== undefined) {
		return policy.includes(parsed.hostname)
			? parsed
			: {
					refused:
						`${parsed.hostname} is not among the responders this application allows` +
						` (${policy.join(", ") || "none"})`,
				};
	}

	// OCSP is defined over HTTP. Anything else is a scheme somebody wanted the
	// server to speak, not a way to ask about a certificate.
	if (parsed.protocol !== "http:" && parsed.protocol !== "https:") {
		return {
			refused: `the certificate names a ${parsed.protocol.replace(":", "")} responder, which is not asked`,
		};
	}
	if (isPrivateHost(parsed.hostname)) {
		return {
			refused:
				`the certificate names ${parsed.hostname}, an address on this network — ` +
				"a document does not get to choose which of your own services is contacted. " +
				"Name it in `allowedResponders` if that really is your authority.",
		};
	}
	return parsed;
}
