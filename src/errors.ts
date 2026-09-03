/**
 * VellumError — structured error for PDF operations.
 */
export class VellumError extends Error {
	readonly code: string;

	constructor(code: string, message: string, options?: ErrorOptions) {
		super(message, options);
		this.name = "VellumError";
		// One namespace, one prefix. `E_` is what every framework code carries,
		// and the package name after it says which package raised it. A code
		// that already starts with `E_` passes through untouched, so a shared
		// identifier keeps its exact spelling.
		this.code = code.startsWith("E_") ? code : `E_VELLUM_${code}`;
	}

	override toString(): string {
		return `${this.name} [${this.code}]: ${this.message}`;
	}
}
