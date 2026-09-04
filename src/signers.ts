/**
 * Signers that need nothing but a key you hold.
 *
 * A signer bound to a certified provider is not here and should not be: it
 * belongs to whoever has the account, as an adapter of its own. This package
 * ships the contract and the one implementation that has no vendor behind it.
 */

import { VellumError } from "./errors.js";
import {
	attachTimestampNative,
	signCmsNative,
	timestampQueryNative,
} from "./native.js";
import type { Signer } from "./Vellum.js";

/** A key you hold, and the certificate that vouches for it. */
export interface Pkcs8SignerOptions {
	/** The private key, PKCS#8 DER. */
	key: Buffer;
	/** The signer's certificate, DER. */
	certificate: Buffer;
	/**
	 * The rest of the chain, DER, nearest issuer first.
	 *
	 * Worth supplying: a verifier that has to go and find the issuers itself
	 * will often decide it cannot.
	 */
	chain?: ReadonlyArray<Buffer>;
}

/**
 * Sign with a key held by the application.
 *
 * ```ts
 * // config/vellum.ts
 * signers: {
 *   internal: pkcs8Signer({
 *     key: readFileSync(app.makePath('storage/signing.key.der')),
 *     certificate: readFileSync(app.makePath('storage/signing.crt.der')),
 *   }),
 * }
 * ```
 *
 * This is an *advanced* signature: it proves the document has not changed
 * since a particular key signed it. Whether that is enough is a question about
 * the document, not about the code — where the law requires a qualified
 * signature, the key has to live with a certified provider, and that is an
 * adapter rather than this.
 *
 * PKCS#8 and DER rather than a `.p12` bundle: reading PKCS#12 in Rust is not
 * something to put underneath a signature, and
 * `openssl pkcs12 -in bundle.p12 -nodes` gets you here in one command.
 */
export function pkcs8Signer(options: Pkcs8SignerOptions): Signer {
	if (options.key.length === 0) {
		throw new VellumError("SIGNER_INVALID", "The signing key is empty.");
	}
	if (options.certificate.length === 0) {
		throw new VellumError("SIGNER_INVALID", "The certificate is empty.");
	}

	const certificates = [options.certificate, ...(options.chain ?? [])];
	return {
		async sign(digest: Buffer): Promise<Buffer> {
			return signCmsNative(
				digest,
				options.key,
				certificates,
				new Date().toISOString(),
			);
		},
	};
}

/** Where to ask for a timestamp, and how. */
export interface TimestampOptions {
	/** The authority's RFC 3161 endpoint. */
	url: string;
	/** Anything the authority needs, such as an Authorization header. */
	headers?: Record<string, string>;
	/** How long to wait before giving up. Default 10 seconds. */
	timeoutMs?: number;
}

/**
 * Add a trusted timestamp to whatever `inner` signs.
 *
 * ```ts
 * signers: {
 *   internal: timestamped(pkcs8Signer({ key, certificate }), {
 *     url: 'https://freetsa.org/tsr',
 *   }),
 * }
 * ```
 *
 * A signature proves a document has not changed since a key signed it, not
 * *when*. Once the signing certificate expires, a verifier cannot tell a
 * signature made while it was valid from one forged afterwards, and stops
 * accepting it. For a document kept for years — which is most of the documents
 * anyone bothers to sign — a timestamp is what keeps it verifiable.
 *
 * It wraps any signer, so it works over a provider's as well as the local one.
 * The token goes on as an unsigned attribute, which is what lets it be added
 * without disturbing the signature.
 *
 * The signature grows by a few kilobytes, so a document prepared with a tight
 * `capacity` may need a larger one.
 */
export function timestamped(inner: Signer, options: TimestampOptions): Signer {
	return {
		async sign(digest: Buffer): Promise<Buffer> {
			const cms = await inner.sign(digest);
			const { query, nonce } = timestampQueryNative(cms);

			let answer: Response;
			try {
				answer = await fetch(options.url, {
					method: "POST",
					headers: {
						"content-type": "application/timestamp-query",
						...options.headers,
					},
					body: new Uint8Array(query),
					signal: AbortSignal.timeout(options.timeoutMs ?? 10_000),
				});
			} catch (error) {
				throw new VellumError(
					"TIMESTAMP_UNREACHABLE",
					`The timestamp authority at ${options.url} could not be reached.`,
					{ cause: error },
				);
			}

			if (!answer.ok) {
				throw new VellumError(
					"TIMESTAMP_REFUSED",
					`The timestamp authority at ${options.url} answered ${answer.status}.`,
				);
			}

			return attachTimestampNative(
				cms,
				Buffer.from(await answer.arrayBuffer()),
				nonce,
			);
		},
	};
}
