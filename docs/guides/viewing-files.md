# Viewing images, PDFs and documents

Not every file in a project is code. DaCode opens these in a tab of their own, the same way it
opens a source file: from the tree, from <kbd>ctrl</kbd>+<kbd>p</kbd>, or as `dacode photo.png`.

| Kind | Files | What you see |
|---|---|---|
| Images | `.png` `.jpg` `.jpeg` `.gif` `.webp` `.bmp` `.ico` `.tif` `.tiff` | The picture, centred, at its own size or fitted to the tab if it is larger. Transparency shows a checkerboard. |
| PDF | `.pdf` | The pages one under another, fitted to the tab's width. |
| Office documents | `.docx` `.xlsx` `.pptx` `.odt` `.ods` `.odp` | The text and tables, read only. Paragraphs are lines, table cells are separated by `│`, and each slide or sheet gets a heading. |
| SVG | `.svg` | Opens as **text**, because an SVG is something you edit. **File ▸ show as picture / as text** (`preview_file` in the command palette) switches between the two. |

A chip at the bottom right says what is shown and how large: `1920 × 1080 · 50%`, or
`page 3 of 12 · 125%`.

## Moving around

| Key or gesture | What it does |
|---|---|
| wheel, <kbd>↑</kbd> <kbd>↓</kbd>, <kbd>j</kbd> <kbd>k</kbd> | scroll |
| <kbd>shift</kbd>+wheel, <kbd>←</kbd> <kbd>→</kbd>, <kbd>h</kbd> <kbd>l</kbd> | scroll sideways, when zoomed past the tab's width |
| <kbd>page down</kbd>, <kbd>space</kbd> · <kbd>page up</kbd> | a screen at a time |
| <kbd>home</kbd>, <kbd>g</kbd> · <kbd>end</kbd>, <kbd>G</kbd> | first page · last page |
| <kbd>ctrl</kbd>+wheel, <kbd>+</kbd> <kbd>-</kbd> | zoom the picture |
| <kbd>0</kbd> | back to fitting the tab |

<kbd>ctrl</kbd>+<kbd>=</kbd> still zooms the whole interface, as it does everywhere else.

## What it does not do

- **Nothing is ever written.** A viewed file's tab cannot be edited, and
  <kbd>ctrl</kbd>+<kbd>s</kbd> says so rather than saving, so a PDF or a photo can never be
  overwritten from here.
- **Office documents are not laid out.** Their pictures, fonts, columns and styles are not shown,
  and the first line of the tab says so. Formulas in a spreadsheet show the value last saved with
  the file.
- **Encrypted PDFs do not open**, and say so. An animated GIF shows its first frame.
- Very large images are scaled down to 8192 pixels on their longest side when they are read, and a
  PDF page is rendered at most 4096 pixels wide; zooming past that magnifies the raster.

Everything is read and rendered off the editor's own thread, so a large scan opening, or a long
PDF scrolling faster than it renders, never makes typing elsewhere wait: pages you have scrolled to
appear as white sheets until their raster arrives.
