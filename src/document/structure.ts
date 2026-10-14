/**
 * pdfkit's logical structure: elements made with `struct()`, holding marked
 * content, links and other elements, attached to the document with
 * `addStructure()`.
 *
 * A child given as a function is content drawn inside the element: it runs
 * once the element is attached, between a `markStructureContent()` and an
 * `endMarkedContent()`, as in pdfkit.
 */

import { VellumError } from "../errors.js";

/** Content marked with `markStructureContent()`. */
export class PdfStructureContent {
	/** The engine's handle. */
	readonly id: number;

	constructor(id: number) {
		this.id = id;
	}
}

/** A link, as a structure element holds it. */
export class PdfAnnotationReference {
	readonly id: number;

	constructor(id: number) {
		this.id = id;
	}
}

export interface StructureOptions {
	title?: string;
	lang?: string;
	/** Alternate text: what a figure or a formula says. */
	alt?: string;
	/** The expansion of an abbreviation. */
	expanded?: string;
	/** The text the content stands for. */
	actual?: string;
	/** For a table header cell. */
	scope?: "Row" | "Column" | "Both";
	rowSpan?: number;
	colSpan?: number;
}

export type StructureChild =
	| PdfStructureElement
	| PdfStructureContent
	| PdfAnnotationReference
	| (() => void);

/** What an element needs from its document. */
export interface StructureHost {
	markStructureContent(type: string): PdfStructureContent;
	endMarkedContent(): unknown;
	enter(element: PdfStructureElement): PdfStructureElement | undefined;
	leave(previous: PdfStructureElement | undefined): void;
}

function isChild(value: unknown): value is StructureChild {
	return (
		value instanceof PdfStructureElement ||
		value instanceof PdfStructureContent ||
		value instanceof PdfAnnotationReference ||
		typeof value === "function"
	);
}

function isChildList(value: unknown): value is readonly StructureChild[] {
	return Array.isArray(value) && value.every(isChild);
}

export class PdfStructureElement {
	readonly type: string;
	readonly options: StructureOptions;
	readonly #host: StructureHost;
	readonly #children: Array<
		| PdfStructureElement
		| PdfStructureContent
		| PdfAnnotationReference
		| (() => void)
	> = [];
	#parent: PdfStructureElement | undefined;
	#attached = false;
	#ended = false;

	constructor(
		host: StructureHost,
		type: string,
		options?: StructureOptions | StructureChild | readonly StructureChild[],
		children?: StructureChild | readonly StructureChild[],
	) {
		this.#host = host;
		this.type = type;
		let given = children;
		if (isChildList(options) || isChild(options)) {
			given = options;
			this.options = {};
		} else {
			this.options = options ?? {};
		}
		if (given !== undefined) {
			const list = isChildList(given) ? given : [given];
			for (const child of list) this.add(child);
			this.end();
		}
	}

	add(child: StructureChild): this {
		if (this.#ended) {
			throw new VellumError(
				"INVALID_STRUCTURE",
				"Cannot add child to already-ended structure element",
			);
		}
		if (child instanceof PdfStructureElement) {
			child.#adopt(this);
			if (this.#attached) child.attach();
		}
		this.#children.push(
			typeof child === "function" && this.#attached ? this.#run(child) : child,
		);
		return this;
	}

	end(): this {
		if (this.#ended) return this;
		for (const child of this.#children) {
			if (child instanceof PdfStructureElement) child.end();
		}
		this.#ended = true;
		return this;
	}

	#adopt(parent: PdfStructureElement): void {
		if (this.#parent !== undefined) {
			throw new VellumError(
				"INVALID_STRUCTURE",
				"Structure element added to more than one parent",
			);
		}
		this.#parent = parent;
	}

	/** Attached to the document: its deferred content is drawn now. */
	attach(): void {
		if (this.#attached) return;
		this.#attached = true;
		this.#children.forEach((child, index) => {
			if (child instanceof PdfStructureElement) child.attach();
			if (typeof child === "function") this.#children[index] = this.#run(child);
		});
	}

	#run(closure: () => void): PdfStructureContent {
		const content = this.#host.markStructureContent(this.type);
		const previous = this.#host.enter(this);
		// Open while its content is drawn, as in pdfkit: a link drawn there
		// is added to it.
		const wasEnded = this.#ended;
		this.#ended = false;
		try {
			closure();
		} finally {
			this.#ended = wasEnded;
			this.#host.endMarkedContent();
			this.#host.leave(previous);
		}
		return content;
	}

	get children(): ReadonlyArray<
		| PdfStructureElement
		| PdfStructureContent
		| PdfAnnotationReference
		| (() => void)
	> {
		return this.#children;
	}

	get isAttached(): boolean {
		return this.#attached;
	}
}

/** The tree flattened for the engine: elements, then their children in order. */
export function flattenStructure(roots: readonly PdfStructureElement[]): {
	nodes: Array<{
		kind: string;
		title?: string;
		lang?: string;
		alt?: string;
		expanded?: string;
		actual?: string;
		scope?: string;
		rowSpan?: number;
		colSpan?: number;
	}>;
	children: Array<{
		parent: number;
		node?: number;
		content?: number;
		annotation?: number;
	}>;
} {
	const nodes: ReturnType<typeof flattenStructure>["nodes"] = [];
	const children: ReturnType<typeof flattenStructure>["children"] = [];
	const visit = (element: PdfStructureElement): number => {
		const index = nodes.length;
		nodes.push({ kind: element.type, ...element.options });
		for (const child of element.children) {
			if (child instanceof PdfStructureElement) {
				children.push({ parent: index, node: visit(child) });
			} else if (child instanceof PdfStructureContent) {
				children.push({ parent: index, content: child.id });
			} else if (child instanceof PdfAnnotationReference) {
				children.push({ parent: index, annotation: child.id });
			}
			// A function never attached was never drawn: it holds nothing.
		}
		return index;
	};
	for (const root of roots) visit(root);
	return { nodes, children };
}
