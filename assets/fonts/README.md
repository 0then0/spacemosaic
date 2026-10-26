# Bundled fonts

All application fonts are embedded for offline use. No runtime font download or
system-font discovery is required. Native system dialogs use platform fonts.

## Inter

[Inter 4.1](https://github.com/rsms/inter/releases/tag/v4.1) supplies the main UI
fonts: Regular 400 for body text and buttons, Medium 500 for headings. The
original static TTF files are taken from `extras/ttf` in the official
`Inter-4.1.zip` release, without modification. Both include Latin and Cyrillic.
The license is SIL Open Font License 1.1; see `Inter-OFL.txt`.

- `Inter-Regular.ttf`: 411,640 bytes; SHA-256
  `40d692fce188e4471e2b3cba937be967878f631ad3ebbbdcd587687c7ebe0c82`.
- `Inter-Medium.ttf`: 417,300 bytes; SHA-256
  `97ad806f526e41546d46365bb3a393145f75b7b1568913db74549ad8b8dba872`.

Together these add 828,940 uncompressed bytes to the embedded assets. Hack,
provided by eframe's default font bundle, remains the primary monospace font.
The other eframe defaults remain available as fallback fonts.

## Noto Sans JP

Noto Sans JP variable TrueType, SIL Open Font License 1.1; see `OFL.txt`.
Original upstream font, not renamed or modified. It is bundled into the Rust
binary for offline Japanese UI and file names. It is the final fallback for
body text, medium-weight headings, and monospace values.
The renderer explicitly selects the variable font's regular weight (`wght=400`).

Source: https://github.com/google/fonts/tree/66a36c8c94b1a5d992ee4e7f392fccfe4945767c/ofl/notosansjp

- File: `NotoSansJP[wght].ttf`, stored locally as `NotoSansJP.ttf`.
- Uncompressed size: 9,589,900 bytes, included once per linked binary.
- SHA-256: `c2f3b4d463500a2ddcd3849cded1fceeb9fd6d1c32e6cbecd568453ba50fc68f`.

No operating-system font discovery or network download is required at runtime.
The full font is bundled to preserve Japanese coverage offline. Mobile raster
quality still requires device verification. The font license must accompany
distributed binaries. Other scripts beyond these fonts are not guaranteed.
