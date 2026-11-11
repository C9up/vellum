import { describe, expect, it } from "vitest";
import { askEachOnce } from "../../src/responder.js";

describe("asking about revocation", () => {
	it("asks once per distinct certificate", async () => {
		const asked: string[] = [];
		const answers = await askEachOnce(
			["a", "b", "a", "a", "b"],
			async (key) => {
				asked.push(key);
				return `answer ${key}`;
			},
			() => "not asked",
		);
		expect(asked.sort()).toEqual(["a", "b"]);
		expect(answers.get("a")).toBe("answer a");
	});

	it("asks a few at a time, never all at once", async () => {
		let running = 0;
		let most = 0;
		const keys = Array.from({ length: 12 }, (_, index) => `k${index}`);
		await askEachOnce(
			keys,
			async () => {
				running++;
				most = Math.max(most, running);
				await new Promise((resolve) => setTimeout(resolve, 5));
				running--;
				return "answered";
			},
			() => "not asked",
			{ concurrency: 4, limit: 100 },
		);
		expect(most).toBe(4);
	});

	it("asks about a bounded number of certificates, the rest are not asked", async () => {
		const keys = Array.from({ length: 100 }, (_, index) => `k${index}`);
		let calls = 0;
		const answers = await askEachOnce(
			keys,
			async () => {
				calls++;
				return "answered";
			},
			(limit) => `not asked past ${limit}`,
			{ limit: 16 },
		);
		expect(calls).toBe(16);
		expect(answers.get("k15")).toBe("answered");
		expect(answers.get("k99")).toBe("not asked past 16");
	});
});
