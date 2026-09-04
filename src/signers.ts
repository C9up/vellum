/**
 * Signers that need nothing but a key you hold.
 *
 * A signer bound to a certified provider is not here and should not be: it
 * belongs to whoever has the account, as an adapter of its own. This package
 * ships the contract and the one implementation that has no vendor behind it.
 */

import { VellumError } from "./errors.js";
import { signCmsNative } from "./native.js";
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
