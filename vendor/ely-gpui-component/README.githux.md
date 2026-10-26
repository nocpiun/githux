# Ely source patch for the Githux reader

Upstream: https://git.zacharyzhang.com/ZacharyZhang-NY/Ely-GPUI-Components.git
Baseline: `da3fdcaf39f2fa85ce1dd9cad07c3c81cf1dd721` (unchanged Ely version 0.1.1 and GPUI 0.3.8).
Only this crate is included. `Cargo.toml` strips references to upstream gallery examples whose sources are not build inputs of the library; `src/`, `assets/` and the MIT `LICENSE` are retained.

Changes:

* Expose the existing `MarkdownDocument` `ListState` and a scroll-notification hook so the reader binds its scrollbar without reconstructing the virtualized list.
* `set_reader_inline` enables read-only continuous paragraph layout in `render/inline.rs`: the mono font, padding, wrapping, rounded boxes and pointer hit testing use the same shaped glyph advances. Source text and copied bytes are unchanged.
* Fence language labels move up 8px locally; code text is not moved. Test-only layout snapshots expose actual geometry/fonts.
* Editing modes keep Ely's existing paragraph and point-to-source behavior.
* Preserve hard breaks, intrinsic table widths and pointer-based glyph link hits. Index glyph positions once for dense paragraphs and skip painting offscreen rows.
* Repository reader pictures reject filesystem and data sources before any loader is built. Default non-reader rendering keeps upstream behavior.

Per-file diffs are in `patches/`. Update against the same upstream commit; do not edit a machine's Cargo cache.
