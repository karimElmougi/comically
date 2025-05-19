# Comically

A minimal comic book converter for Kindle devices. Converts CBZ/CBR files to MOBI format.

## Features

- Simple command-line interface
- Extracts CBZ comic archives
- Optimizes images for Kindle display
- Creates EPUB from comic pages
- Converts to MOBI using Amazon's KindleGen

## Prerequisites

#### Rust
see https://www.rust-lang.org/tools/install

#### KindleGen 
On Windows and macOS, install [Kindle Previewer 3 (KP3)](https://www.amazon.com/Kindle-Previewer/b?ie=UTF8&node=21381691011). KindleGen is automatically included.

## Installation

```bash
git clone https://github.com/nicoburniske/comically.git
cd comically
cargo build --release
```

The executable will be available at `target/release/comically`.

## Usage

#### CLI options

```shell
cargo run --release -- --help
```

```shell
Usage: comically [OPTIONS] <INPUT>...

Arguments:
  <INPUT>...  the input files to process. can be a directory or a file. supports .cbz, .zip, .cbr, .rar files

Options:
  -p, --prefix <PREFIX>          the prefix to add to the title of the comics + the output file
  -m, --manga [<MANGA>]          whether to read the comic from right to left [default: true] [possible values: true, false]
  -q, --quality <QUALITY>        the jpg compression quality of the images, between 0 and 100 [default: 75]
  -b, --brightness <BRIGHTNESS>  brighten the images positive values will brighten the images, negative values will darken them
  -c, --contrast <CONTRAST>      the contrast of the images positive values will increase the contrast, negative values will decrease it
  -t <THREADS>                   the number of threads to use for processing. defaults to the number of logical CPUs
      --crop <CROP>              crop the dead space on each page [default: true] [possible values: true, false]
      --split <SPLIT>            split double pages into two separate pages [default: true] [possible values: true, false]
  -h, --help                     Print help
  -V, --version                  Print version
```

#### Basic usage with file
```bash
cargo run --release -- naruto-volume-1.cbz
```


#### Basic usage with directory 
```bash
cargo run --release -- naruto-complete/
```
