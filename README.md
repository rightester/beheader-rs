# beheader-rs

> A Rust port of [beheader](https://github.com/p2r3/beheader) by p2r3.

This project is a Rust port of the original [beheader](https://github.com/p2r3/beheader) polyglot generator by p2r3. All credit for the original concept and implementation goes to the original author.

A polyglot generator for media files — produce a single file that behaves as an image, video, PDF, HTML page, or ZIP archive depending on its file extension.

## Features

- **Self-contained binary**: All external CLI tool logic has been integrated into the binary itself. No extra executable dependencies required, except `ffmpeg.exe` for video/audio encoding.
- **Decoupled architecture**: Split into a `bin` target (the CLI executable) and a `lib` target (reusable library crate), allowing programmatic use of the polyglot generation logic.
- **Automatic ffmpeg discovery**: The CLI automatically searches for `ffmpeg.exe` in the current directory and system `PATH`.
- **`--preprocessed-mp4`**: Accept a pre-encoded MP4 file as input, completely bypassing the need for `ffmpeg.exe`.
- **`--temp-dir`**: Specify a custom directory for temporary files produced during video processing.
- **WASM/WASI support**: When used as a WASM/WASI module or as a library, `ffmpeg.exe` cannot be invoked — only the `--preprocessed-mp4` parameter is available for video input.
- **Improved HTML embedding**: HTML content is embedded using a pre-processing script that runs before the page is displayed, ensuring more reliable rendering.

## Installation

### From crates.io

```bash
cargo install beheader-rs
```

### From source

```bash
git clone https://github.com/rightester/beheader-rs.git
cd beheader-rs
cargo build --release
```

The binary will be at `target/release/beheader-bin`.

### Pre-built binaries

Download from [GitHub Releases](https://github.com/rightester/beheader-rs/releases).

## Usage

```bash
beheader <output> <image> [video] [options] [appendable...]
```

**Positional arguments:**

| Argument | Description |
|----------|-------------|
| `output` | Path of the resulting polyglot file |
| `image` | Path of the input image file |
| `video` | Path of the input video/audio file (encoded by ffmpeg) |
| `appendable` | Path(s) of files to append without parsing |

**Options:**

| Flag | Description |
|------|-------------|
| `-H, --html <path>` | Path to HTML document |
| `-p, --pdf <path>` | Path to PDF document |
| `-z, --zip <path>` | Path to ZIP-like archive (repeatable) |
| `-e, --extra <path>` | Path to short (<200 byte) file to include near the header |
| `--preprocessed-mp4 <path>` | Path to a pre-encoded MP4 file (H.264 video or AAC audio), skips ffmpeg |
| `--temp-dir <dir>` | Directory for temporary files |

**Examples:**

```bash
# Basic: image + video
beheader output.polyglot image.png video.mp4

# With HTML and PDF
beheader output.polyglot image.png video.mp4 --html page.html --pdf doc.pdf

# Pre-encoded MP4 (no ffmpeg needed)
beheader output.polyglot image.png --preprocessed-mp4 encoded.mp4

# As a library
use beheader::{build_polyglot, PolyglotConfig, convert_image_to_png};
```

Run without arguments for interactive mode.

## As a Library

Add to your `Cargo.toml`:

```toml
[dependencies]
beheader-rs = "0.3"
```

```rust
use beheader::{build_polyglot, PolyglotConfig};

let config = PolyglotConfig {
    png_data: std::fs::read("image.png")?,
    mp4_data: std::fs::read("video.mp4")?,
    html_content: None,
    pdf_data: None,
    extra_data: None,
};

let result = build_polyglot(&config)?;
std::fs::write("output.polyglot", &result.data)?;
```

## License

GPL-3.0 — see [LICENSE](LICENSE) for details.

This project is a port of [beheader](https://github.com/p2r3/beheader) by p2r3, originally licensed under GPL-3.0.

## Acknowledgments

This project was built entirely using the Qwen 3.5 Plus model and [opencode](https://opencode.ai).
