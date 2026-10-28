Caption fonts, bundled into `sf-render` (`text.rs`). SemiBold weight, split into the latin,
latin-ext and cyrillic subsets published by Fontsource; the renderer falls back through the
subsets glyph by glyph.

| file prefix              | family          | license                           |
|--------------------------|-----------------|-----------------------------------|
| `Inter-SemiBold`         | Inter           | SIL OFL 1.1, `OFL-Inter.txt`       |
| `SourceSerif4-SemiBold`  | Source Serif 4  | SIL OFL 1.1, `OFL-SourceSerif4.txt`|
| `JetBrainsMono-SemiBold` | JetBrains Mono  | SIL OFL 1.1, `OFL-JetBrainsMono.txt`|
| `Montserrat-ExtraBold`   | Montserrat      | SIL OFL 1.1, `OFL-Montserrat.txt`  |

Montserrat is ExtraBold (800), the heavy face of the highlight caption style; its Fontsource
WOFF2 subsets were converted to TTF with fontTools.

`Inter-SemiBold-symbols.ttf` is a subset of Inter 4.0 SemiBold (rsms/inter release) with only
the keyboard symbols used by the shortcut overlay (⌘ ⇧ ⌥ ⌃ ⏎ ⌫ ⌦ ⎋ ⇥ arrows, ⇞ ⇟ ↖ ↘ ␣ ⇪),
made with `pyftsubset`. It is the last fallback of every family.
