/**
 * Asking a revocation responder, and deciding whether it may be asked.
 *
 * The address comes out of the certificate inside the document being checked,
 * which is to say: from whoever sent the document. Following it unconditionally
 * turns "check whether this signature is still valid" into "make my server
 * issue an HTTP request wherever this stranger points" — the cloud metadata
 * endpoint, a Redis on loopback, or simply a URL that confirms the document was
 * opened and reveals the address it was opened from.
 *
 * Two gates stand in the way. The application's policy decides which
 * responders may be named at all. The outbound client decides which address
 * is connected to: it checks the address the socket actually connects to,
 * after DNS, so a name that resolves to loopback, or that answers a public
 * address and then a private one, is refused too. A spelling check on the
 * URL could not see either. Redirects are not followed, and the answer is
 * read up to a size an OCSP answer never reaches.
 *
 * Everything refused answers `"unknown"`, never `"good"`: a responder that was
 * not asked has told us nothing.
 */

import type { RevocationAnswer } from "./native.js";

/** Who an application is willing to ask about revocation. */
export type ResponderPolicy =
	| ReadonlyArray<string>
	| ((url: URL) => boolean | Promise<boolean>);

/**
 * What asking a responder needs of an HTTP client: `HttpClient` from
 * `@c9up/ream/http-client`, or anything with its `post`. A client given here
 * is trusted to refuse private addresses the way that one does, and to follow
 * no redirect to another host.
 */
export interface ResponderClient {
	post(
		url: URL,
		options: {
			headers: Record<string, string>;
			body: Uint8Array;
			timeout: number;
			maxResponseBytes: number;
		},
	): Promise<{ status: number; body: Buffer }>;
}

/** The largest answer read, as the engine reads it. */
export const MAX_RESPONDER_ANSWER = 64 * 1024;

/** Why a responder was refused, in a sentence a caller can act on. */
export interface ResponderRefusal {
	refused: string;
}

/** A responder that may be asked, and whether the application named it. */
export interface ResponderPermit {
	url: URL;
	/**
	 * Named by the application rather than merely public: its address may be
	 * on the application's own network.
	 */
	named: boolean;
}

/** The port a URL connects to, written or implied. */
function portOf(url: URL): string {
	return url.port || (url.protocol === "https:" ? "443" : "80");
}

/**
 * Whether a list entry names `url`.
 *
 * An entry is an origin (`https://ocsp.internal:8443`), matched on scheme,
 * host and port; or a bare host (`ocsp.internal`), matched over http or https
 * on their own ports. A host alone used to admit any port and any scheme on
 * it, so naming an authority also opened its Redis.
 */
function entryNames(entry: string, url: URL): boolean {
	if (entry.includes("://")) {
		try {
			const origin = new URL(entry);
			return (
				origin.protocol === url.protocol &&
				origin.hostname === url.hostname &&
				portOf(origin) === portOf(url)
			);
		} catch {
			return false;
		}
	}
	return (
		entry.toLowerCase() === url.hostname &&
		(url.protocol === "http:" || url.protocol === "https:") &&
		url.port === ""
	);
}

/**
 * Decide whether `url` may be asked.
 *
 * @param policy What the application allows: a list of hosts or origins, or a
 *   predicate. Absent, any responder over http or https may be named, and the
 *   outbound client refuses every address that is not public.
 */
export async function mayAsk(
	url: string,
	policy?: ResponderPolicy,
): Promise<ResponderPermit | ResponderRefusal> {
	let parsed: URL;
	try {
		parsed = new URL(url);
	} catch {
		return { refused: `the certificate names "${url}", which is not a URL` };
	}

	if (typeof policy === "function") {
		return (await policy(parsed))
			? { url: parsed, named: true }
			: { refused: `the configured policy refused ${parsed.origin}` };
	}
	if (policy !== undefined) {
		return policy.some((entry) => entryNames(entry, parsed))
			? { url: parsed, named: true }
			: {
					refused:
						`${parsed.origin} is not among the responders this application allows` +
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
	return { url: parsed, named: false };
}

/** What vellum builds ream's client with. */
export interface GuardedClientOptions {
	maxRedirects: number;
	/** `host:port` entries exempt from the public-address rule. */
	allowHosts: string[];
}

/** A client vellum built, and so closes once it has asked. */
export interface GuardedClient extends ResponderClient {
	close(): void;
}

/**
 * The part of `@c9up/ream/http-client` vellum uses: its `HttpClient`, which
 * refuses private addresses at connect time.
 */
export interface ClientModule {
	HttpClient: new (options: GuardedClientOptions) => GuardedClient;
}

/**
 * Held in a constant so neither the compiler nor a bundler resolves it ahead
 * of time: ream is an optional peer, and an installed ream older than this
 * subpath must not stop the whole package from loading.
 */
const REAM_CLIENT_MODULE = "@c9up/ream/http-client";

/** Whether `value` carries a constructor whose instances can post and close. */
function isClientModule(value: unknown): value is ClientModule {
	if (typeof value !== "object" || value === null) return false;
	if (!("HttpClient" in value) || typeof value.HttpClient !== "function") {
		return false;
	}
	const prototype: unknown = value.HttpClient.prototype;
	return (
		typeof prototype === "object" &&
		prototype !== null &&
		"post" in prototype &&
		typeof prototype.post === "function" &&
		"close" in prototype &&
		typeof prototype.close === "function"
	);
}

/** Import failures that mean "ream's client is not available here". */
const ABSENT_MODULE_CODES = new Set([
	"ERR_MODULE_NOT_FOUND",
	"ERR_PACKAGE_PATH_NOT_EXPORTED",
]);

/**
 * ream's client module out of whatever `load` returns, or `undefined` when it
 * is not there: ream not installed, installed without this subpath, or
 * exporting something else under it. Any other failure is not a missing
 * module, and propagates.
 */
export async function readClientModule(
	load: () => Promise<unknown>,
): Promise<ClientModule | undefined> {
	let loaded: unknown;
	try {
		loaded = await load();
	} catch (error) {
		if (
			error instanceof Error &&
			"code" in error &&
			typeof error.code === "string" &&
			ABSENT_MODULE_CODES.has(error.code)
		) {
			return undefined;
		}
		throw error;
	}
	return isClientModule(loaded) ? loaded : undefined;
}

let clientModule: Promise<ClientModule | undefined> | undefined;

/**
 * ream's outbound client, when it is installed. It is an optional peer: an
 * application that never checks revocation does not need it.
 */
function loadClientModule(): Promise<ClientModule | undefined> {
	clientModule ??= readClientModule(() => import(REAM_CLIENT_MODULE));
	return clientModule;
}

function redirected(url: URL): RevocationAnswer {
	return {
		status: "unknown",
		detail: `the responder at ${url.origin} redirected, and a redirect is not followed: it would let the document's responder choose another address`,
	};
}

/** Why an outbound request failed, if the client said. */
function reasonOf(error: unknown): string | undefined {
	if (!(error instanceof Error) || !("reason" in error)) return undefined;
	return typeof error.reason === "string" ? error.reason : undefined;
}

/**
 * POST an OCSP request to a responder `mayAsk` admitted, and hand back its
 * answer, or why there is none.
 */
export async function askResponder(
	permit: ResponderPermit,
	query: Uint8Array,
	options: {
		timeoutMs: number;
		client?: ResponderClient;
		/** Where ream's client comes from when no `client` is given. */
		clientModule?: () => Promise<ClientModule | undefined>;
	},
): Promise<Buffer | RevocationAnswer> {
	const { url } = permit;
	let client: ResponderClient;
	let owned: GuardedClient | undefined;
	if (options.client !== undefined) {
		client = options.client;
	} else {
		// Without ream's client there is nothing that refuses private
		// addresses, so nothing is asked: a plain fetch would go wherever the
		// document points.
		const module = await (options.clientModule ?? loadClientModule)();
		if (module === undefined) {
			return {
				status: "unknown",
				detail:
					"checking revocation needs an outbound client that refuses private addresses: install @c9up/ream 0.2.29 or later, or pass httpClient",
			};
		}
		owned = new module.HttpClient({
			maxRedirects: 0,
			// A responder the application named may sit on its own network.
			allowHosts: permit.named ? [`${url.hostname}:${portOf(url)}`] : [],
		});
		client = owned;
	}

	try {
		const answer = await client.post(url, {
			headers: { "content-type": "application/ocsp-request" },
			body: query,
			timeout: options.timeoutMs,
			maxResponseBytes: MAX_RESPONDER_ANSWER,
		});
		if (answer.status >= 300 && answer.status < 400) return redirected(url);
		if (answer.status < 200 || answer.status >= 300) {
			return {
				status: "unknown",
				detail: `the responder at ${url.origin} answered ${answer.status}`,
			};
		}
		return answer.body;
	} catch (error) {
		const reason = reasonOf(error);
		const message = error instanceof Error ? error.message : String(error);
		if (reason === "privateAddress") {
			return {
				status: "unknown",
				detail:
					`the certificate names ${url.hostname}, an address on this network (${message}) — ` +
					"a document does not get to choose which of your own services is contacted. " +
					"Name it in `allowedResponders` if that really is your authority.",
			};
		}
		// A client given by the application may follow redirects; its own rules
		// then refused this one.
		if (reason === "redirect") return redirected(url);
		if (reason === "tooLarge") {
			return {
				status: "unknown",
				detail: `the responder at ${url.origin} answered more than ${MAX_RESPONDER_ANSWER / 1024} KiB, which no OCSP answer needs`,
			};
		}
		return {
			status: "unknown",
			detail: `the responder at ${url.origin} could not be reached: ${message}`,
		};
	} finally {
		owned?.close();
	}
}
