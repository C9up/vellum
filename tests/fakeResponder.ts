/**
 * A revocation client that never touches the network, for tests.
 *
 * It answers what a test registered, records what it was asked, and fails the
 * way an outbound client reports a refusal: an error carrying a `reason`.
 * Which addresses a real client refuses is that client's business, tested
 * where it lives; these tests are about what vellum asks and how it reads the
 * answer.
 */

import type { ResponderClient } from "../src/index.js";

/** A failure the way an outbound client reports one. */
export class ClientFailure extends Error {
	readonly reason: string;

	constructor(reason: string, message: string) {
		super(message);
		this.reason = reason;
	}
}

/** What a registered responder answers. */
export interface FakeAnswer {
	/** Default 200. */
	status?: number;
	body?: string | Uint8Array;
}

/** A request the fake received, with every option it was given. */
export interface AskedRequest {
	url: string;
	headers: Record<string, string>;
	body: Uint8Array;
	timeout: number;
	maxResponseBytes: number;
}

export interface FakeResponder extends ResponderClient {
	readonly requests: AskedRequest[];
	/** Answer `url` with `answer`, or fail with `answer` when it is an error. */
	on(url: string, answer: FakeAnswer | Error): this;
}

export function fakeResponder(): FakeResponder {
	const answers = new Map<string, FakeAnswer | Error>();
	return {
		requests: [],
		on(url, answer) {
			answers.set(url, answer);
			return this;
		},
		async post(url, options) {
			this.requests.push({ url: url.href, ...options });
			const answer = answers.get(url.href);
			if (answer === undefined) {
				throw new ClientFailure("unreachable", `nothing answers ${url.href}`);
			}
			if (answer instanceof Error) throw answer;
			const body = Buffer.from(answer.body ?? "");
			if (body.length > options.maxResponseBytes) {
				throw new ClientFailure(
					"tooLarge",
					`more than ${options.maxResponseBytes} bytes`,
				);
			}
			return { status: answer.status ?? 200, body };
		},
	};
}
