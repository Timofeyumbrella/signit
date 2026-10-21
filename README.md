# signit

A small macOS app for signing PDFs: open a PDF, draw on it with the trackpad, save a signed copy.
The ink is written into the page as vector paths, so it stays sharp at any zoom and cannot be
moved or deleted like an annotation could. The PDF you opened is never changed.

## Setup (once)

1. Install Rust: <https://rustup.rs>
2. In this folder, download the PDF engine (Google's PDFium, the one Chrome uses):

   ```sh
   ./scripts/fetch-pdfium.sh
   ```

   It puts `libpdfium.dylib` in `vendor/pdfium/lib/` and checks its SHA-256.

3. Install the `signit` command into `~/.cargo/bin` (already on your PATH with Rust):

   ```sh
   cargo install --path .
   ```

   Run this again after changing the code. The installed command loads PDFium from this
   folder's `vendor/`, so keep the folder where it is (or re-install after moving it).

## Run

```sh
signit ~/Downloads/contract.pdf
signit                      # then open a PDF with Cmd+O, or drop one on the window
```

The terminal waits until the window is closed. To get the prompt back right away, add this to
`~/.zshrc`:

```sh
signit() { command signit "$@" &>/dev/null &! }
```

(Errors then no longer show in the terminal: a mistyped path just opens nothing.)

While working on the code, `cargo run --release -- file.pdf` runs it without installing.

## Use

| Do this | To |
|---|---|
| Click and drag on a page | Draw (a click alone makes a dot) |
| Two-finger scroll | Move around |
| Pinch, or Cmd + two-finger scroll | Zoom around the pointer |
| Cmd+= / Cmd+- / Cmd+0 | Zoom in / out / fit the page width |
| P / E | Pen / eraser (the eraser removes whole strokes) |
| Cmd+Z / Shift+Cmd+Z | Undo / redo |
| Cmd+O / Cmd+S | Open / save a signed copy |

The colour swatches and the width slider (in pt, like the PDF) set the pen. A two-finger click
(right click) never draws.

## How saving works

Save As suggests `<name>-signed.pdf` next to the original and refuses to overwrite the original.
Before anything is written, signit:

1. draws your strokes into a fresh copy of the PDF in memory, shielding them from drawing state
   the page leaves behind (a leftover clip or dash pattern would otherwise cut or dash your ink);
2. renders every page you drew on, before and after, and compares them. If anything outside your
   drawing changed (PDFium can drop a font from some old PDFs), nothing is saved and you are told;
3. checks that your ink is actually visible. If it is under a form field (PDF viewers draw form
   fields on top of the page), you can flatten that page's form fields, save anyway, or cancel.

The file is then written to a temporary file and renamed into place, so a failed save never
leaves a half-written PDF.

- **Password-protected PDFs** open after you type the password; the copy keeps the encryption.
- **Digitally signed PDFs** (for example by an e-signature service): you are warned first. Your
  copy no longer carries a valid digital signature, while the original keeps it.

## Limits

- Cmd+H and the app menu are off: macOS's default menu would quit on Cmd+Q without asking about
  unsaved strokes.
- Open files with Cmd+O or by dropping them on the window. Finder's "Open With" does not work.
- Pages are drawn up to 6144 pixels on their long side, so at very high zoom they get softer.

## Tests

```sh
cargo test                                           # unit, PDF and UI tests (needs PDFium)
cargo test --test screenshots -- --ignored           # renders the UI to target/screenshots/
```

`tests/pdf.rs` saves strokes into PDFs that broke simpler approaches (rotated and cropped pages,
inherited page boxes, leftover graphics state, form fields, encryption) and checks the pixels of
the result. `tests/ui.rs` drives the app with simulated trackpad and keyboard input.
