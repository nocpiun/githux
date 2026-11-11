# SVG regression fixtures

The SVG geometry and example labels in this directory are original minimal test data.
They reproduce supported CSS/data-image shapes without redistributing public Profile artwork.

`embedded-font.woff` is a test-only WOFF1 subset of the repository's
`assets/fonts/google-sans-code/GoogleSansCode.ttf` (v7.001), under the
[SIL Open Font License 1.1](../../../../assets/fonts/google-sans-code/OFL.txt).
The original copyright/license name records are retained. The derivative's family,
full name and PostScript name are `Githux SVG Fixture` / `GithuxSVGFixture`.
It is included only by `#[cfg(test)]` code and is not an application font.

The subset contains space, `0123`, `ABCD`, `I` and `W` (including required layout
closure glyphs). It was produced with fontTools' subsetter, preserving the source
outlines/advances and variable-font tables, renaming name IDs 1/4/6, then saving
with `font.flavor = "woff"`. Tests compare the rendered glyphs with the original
bundled face, so selecting a fallback face cannot pass accidentally.

Size: 5,312 bytes. SHA-256:
`20de0d77199053950d57640b4ce7a4b2063a0532a5f7f57d7ce7369958c6b7aa`.
