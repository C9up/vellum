/**
 * `ream configure @c9up/vellum` — wire PDF rendering in one command.
 *
 * The provider alone is not enough: it reads `config/vellum.ts`, and a package
 * registered without one renders with defaults the application never chose.
 * Writing both together is what makes `ream add` mean installed AND working.
 */

interface Codemods {
	addProvider(importPath: string): Promise<void>;
	addEnvVars(vars: Record<string, string>): Promise<void>;
	writeFile(
		filePath: string,
		content: string,
		options?: { force?: boolean },
	): Promise<void>;
}

export async function configure(codemods: Codemods): Promise<void> {
	// The config below reads these, so they are declared here. Writing the file
	// without them leaves an application whose config asks the environment for
	// something nothing ever put there.
	await codemods.addEnvVars({
		VELLUM_FORMAT: "png",
	});

	await codemods.addProvider("@c9up/vellum/provider");
	await codemods.writeFile(
		"config/vellum.ts",
		`import { defineConfig } from '@c9up/vellum'
import env from '#start/env'

export default defineConfig({
  // Defaults for every render. Any call can override them.
  format: env.get('VELLUM_FORMAT', 'png'),

  // 1 is 72 DPI — the page's natural size. Raise it for print, or set
  // \`width\` instead when what matters is the pixel width of a preview.
  scale: 1,

  // Only read when the format is 'jpeg'.
  quality: 82,

  // A PDF paints no background of its own, so rendering it transparent
  // makes black text invisible over a dark viewer.
  background: '#ffffff',
})`,
	);
}
