# Bundled print fonts

Pinned static fonts for Bob note snapshots. Source Serif 4 Text supplies serif body
text, Inter supplies sans-serif text and structure (including the Inter Display title
face), JetBrains Mono supplies code, and Noto Emoji supplies monochrome fallback glyphs.
All faces are static OTF/TTF files; no variable font is shipped. Noto Sans Symbols 2 is
not needed for the current snapshot corpus.

Each file is pinned by upstream release and SHA-256. The payload included in the Bob
binary is **5,976,700 bytes** (5.977 MB), below the 6 MB budget.

| File | Family / role | Upstream version | Source | Bytes | SHA-256 |
| ---- | ---- | ---- | ---- | ----: | ---- |
| `SourceSerif4SmText-Regular.otf` | Source Serif 4 Text | 4.005 | https://github.com/adobe-fonts/source-serif/tree/4.005R/OTF | 245,204 | `760327b0f9ef55310f11ed5c955693f4841f9b859407e98570d3654422a34d70` |
| `SourceSerif4SmText-It.otf` | Source Serif 4 Text | 4.005 | https://github.com/adobe-fonts/source-serif/tree/4.005R/OTF | 172,992 | `220a0bf6d2a0c3fde644db2b2b602e725170ac1a90b0ce79994b7598c12980d1` |
| `SourceSerif4SmText-Semibold.otf` | Source Serif 4 Text | 4.005 | https://github.com/adobe-fonts/source-serif/tree/4.005R/OTF | 252,660 | `1e32888469460231d1d43369a1cf88d2f8279571893418e653945171ac91bcb8` |
| `SourceSerif4SmText-SemiboldIt.otf` | Source Serif 4 Text | 4.005 | https://github.com/adobe-fonts/source-serif/tree/4.005R/OTF | 177,964 | `965a1732d7d1b4550d2db3a7fe8aea5c9245fc1e98bb88077d089acd995ac71a` |
| `Inter-Regular.otf` | Inter | 4.1 | https://github.com/rsms/inter/releases/download/v4.1/Inter-4.1.zip | 609,600 | `d4f2b9e148059a15f014cb0f0b8fea8cd11bfa447dd483bedf1b0adc0e2ba799` |
| `Inter-Italic.otf` | Inter | 4.1 | https://github.com/rsms/inter/releases/download/v4.1/Inter-4.1.zip | 589,784 | `3bdf49f72ad2f2959a2619ceef700784504f6d17f502e2e2bc6edce5d43ca631` |
| `Inter-Medium.otf` | Inter | 4.1 | https://github.com/rsms/inter/releases/download/v4.1/Inter-4.1.zip | 627,988 | `6c222ed2c3660524dd0c14fb745e00fee65c5361ab28f60fba6730bf4e648763` |
| `Inter-SemiBold.otf` | Inter | 4.1 | https://github.com/rsms/inter/releases/download/v4.1/Inter-4.1.zip | 629,912 | `0a4d30778fa2dc239368d90fd854b88b27e7ede737450da72686a90eca66f02c` |
| `Inter-SemiBoldItalic.otf` | Inter | 4.1 | https://github.com/rsms/inter/releases/download/v4.1/Inter-4.1.zip | 608,028 | `a350102650a86612efa66e0291315dba24be43e1f3716359c17d365376bea11e` |
| `InterDisplay-SemiBold.otf` | Inter Display | 4.1 | https://github.com/rsms/inter/releases/download/v4.1/Inter-4.1.zip | 625,296 | `991234562ac06b47aefa2ca4d4ff74360a164a5653ec05357816fc4ffe3ca8a2` |
| `JetBrainsMono-Regular.ttf` | JetBrains Mono | 2.304 | https://github.com/JetBrains/JetBrainsMono/tree/v2.304/fonts/ttf | 273,900 | `a0bf60ef0f83c5ed4d7a75d45838548b1f6873372dfac88f71804491898d138f` |
| `JetBrainsMono-Bold.ttf` | JetBrains Mono | 2.304 | https://github.com/JetBrains/JetBrainsMono/tree/v2.304/fonts/ttf | 277,828 | `5590990c82e097397517f275f430af4546e1c45cff408bde4255dad142479dcb` |
| `NotoEmoji-Regular.ttf` | Noto Emoji | 3.006 (Google Fonts v65) | https://fonts.gstatic.com/s/notoemoji/v65/bMrnmSyK7YY-MEu6aWjPDs-ar6uWaGWuob-r0jwv.ttf | 885,544 | `988621dc5c9a75eb6144f28faae30317a8e3421b68b28740747b3d739e2326b8` |

The Inter assets are the static OTF files from the v4.1 release archive. Source Serif 4
Text uses the `SmText` optical-size instances from tag `4.005R`. The Noto Emoji URL is
versioned (`v65`) and returns the static Regular TTF whose name table reports version
3.006. Licenses and copyright notices for all four families are in [OFL.txt](OFL.txt).
