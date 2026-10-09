# tsgo only, no tsserver fallback

Genea gets all type-level language intelligence from one language server: TypeScript 7's native `tsc --lsp`, one process per project. It does not fall back to the JS-based tsserver for features tsgo lacks. tsgo loads projects 8–13× faster than tsserver and speaks standard LSP. A tsserver fallback would mean shipping Node, running a second type checker (roughly double the memory on the reference machine), and merging two servers' answers. In exchange, v1 has no refactors (extract function, move to file, and so on) and only tsgo's few quick fixes: missing imports, implement interface, isolated-declaration annotations, fix-all, and organize/sort/remove-unused imports.

## Consequences

- Refactors come back only as tsgo implements them. Nothing in TS 7.1's plan names them.
- Adding a fallback later is possible, but it would undo the reasons above, so it would need its own decision.
- Frameworks that need tsserver plugins (Vue, Svelte, Astro, MDX, Angular templates) stay unsupported, consistent with Genea's first-class languages.
