# Glossary

**Genea**: The editor itself: a small, fast IDE for building frontend and backend applications in TypeScript.

**Project**: The folder a user opens in Genea. Holds the config at its root. May contain a single package or be a workspace.

**Package**: One unit of code with its own `package.json`.

**Workspace**: A project containing several packages, typically frontend, backend and shared code side by side (a monorepo).
_Avoid_: monorepo (fine in conversation, but "workspace" is the term)

**First-class language**: A language Genea fully understands: TypeScript, TSX, JavaScript and JSX. Only first-class languages get language intelligence.
_Avoid_: supported language (ambiguous; basic file types are also "supported")

**Basic file type**: A non-first-class file Genea edits comfortably but doesn't understand: JSON, CSS, HTML, Markdown, YAML, `.env`. Gets syntax highlighting and basic editing, no language intelligence.

**Language intelligence**: Understanding of code meaning: completions, diagnostics, go-to-definition, find references, rename and refactors. Distinct from syntax highlighting.

**Toolchain**: The set of tools Genea uses to install, run, build, test, lint and format a project, one tool per toolchain role.

**Toolchain role**: One job within the toolchain: runtime, package manager, bundler/dev server, test runner, linter/formatter.

**Blessed tool**: The tool Genea uses for a toolchain role unless the config picks one of a short list of alternatives. The package manager is the exception: it is detected from the project's lockfile.

**Template**: A built-in starting point for a new project: frontend, backend, or full-stack. The full-stack template creates a workspace. Templates are fixed; users cannot add their own.

**Config**: The single, small file at a project's root through which a user adjusts Genea. Everything it omits falls back to built-in defaults. There is no global config, and no plugins or extensions; the config is the only customisation surface.
_Avoid_: settings, preferences, extensions
