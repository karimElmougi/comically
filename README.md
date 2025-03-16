# Comically

A minimal comic book converter for Kindle devices. Converts CBZ files to MOBI format.

## Features

- Simple command-line interface
- Extracts CBZ comic archives
- Optimizes images for Kindle display
- Creates EPUB from comic pages
- Converts to MOBI using Amazon's KindleGen

## Prerequisites

- [Rust](https://www.rust-lang.org/tools/install)
- [KindleGen](https://archive.org/details/kindlegen-2.9) (required for MOBI conversion)

## Installation

```bash
git clone https://github.com/yourusername/comically.git
cd comically
cargo build --release
```

The executable will be available at `target/release/comically`.

## Usage

```bash
# Basic usage
comically input.cbz

# Specify output file
comically input.cbz -o output.mobi

# Keep temporary files for debugging
comically input.cbz --keep-temp
```

## Notes

This is a minimal port of the [Kindle Comic Converter (KCC)](https://github.com/ciromattia/kcc) project, with a focus on simplicity and the core CBZ to MOBI conversion workflow.

## Requirements

- Amazon's KindleGen must be installed and available in your PATH for MOBI conversion.