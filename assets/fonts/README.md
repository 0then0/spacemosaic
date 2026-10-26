# Bundled font

Noto Sans JP variable TrueType, SIL Open Font License 1.1; see `OFL.txt`.
Original upstream font, not renamed or modified. It is bundled into the Rust
binary for offline Japanese UI and file names. Latin and Cyrillic use eframe's
bundled defaults; Noto Sans JP is the final fallback for both UI font families.
The renderer explicitly selects the variable font's regular weight (`wght=400`).

Source: https://github.com/google/fonts/tree/66a36c8c94b1a5d992ee4e7f392fccfe4945767c/ofl/notosansjp

- File: `NotoSansJP[wght].ttf`, stored locally as `NotoSansJP.ttf`.
- Uncompressed size: 9,589,900 bytes, included once per linked binary.
- SHA-256: `c2f3b4d463500a2ddcd3849cded1fceeb9fd6d1c32e6cbecd568453ba50fc68f`.

No operating-system font discovery or network download is required at runtime.
The full font is bundled to preserve Japanese coverage offline. Mobile raster
quality still requires device verification. The font license must accompany
distributed binaries. Other scripts beyond these fonts are not guaranteed.
