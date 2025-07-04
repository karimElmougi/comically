# comically

the fastest manga/comic optimizer for e-readers

![comically splash screen](assets/goku-splash-original.jpg)

## what's this?

comically optimizes manga and comics for e-ink readers. it makes pages display fullscreen without margins, with proper fixed layout support.

why use this? e-ink screens need different processing than lcd screens. comically handles the annoying stuff:
- fixes washed out blacks that make comics hard to read
- removes unnecessary margins 
- uses your device's full resolution
- handles right-to-left manga properly
- aligns two-page spreads correctly

the result? way smaller file sizes (hundreds of mb saved per volume), faster page turns, better battery life. all without visible quality loss on e-ink. 

features:
- multi-threaded processing
- live preview with adjustments
- mouse support
- device presets for e-readers
- smart page splitting & rotation
- saves your last config

## prerequisites

#### rust
see https://www.rust-lang.org/tools/install

#### kindlegen (for awz3/mobi output)
on windows and macos, install [kindle previewer 3](https://www.amazon.com/Kindle-Previewer/b?ie=UTF8&node=21381691011). kindlegen is automatically included.

## installation

```bash
cargo install comically
```

## usage

```bash
comically [directory] [--output path]
```

defaults to current directory if no path provided. output defaults to `{directory}/comically/`.

### supported devices

kindle - paperwhite 11/12, oasis, scribe, basic  
kobo - clara hd/2e, libra 2, sage, elipsa  
remarkable - 2, ipad mini/pro, onyx boox, pocketbook era

### output formats

- **awz3/mobi** - amazon kindle format (REQUIRES KINDLEGEN)
- **epub** - universal e-reader format
- **cbz** - comic book archive (processed/optimized)

## tips

- gamma 1.8-2.2 works great for e-ink
- brightness -10 to -20 for scanned comics
- \"rotate & split\" for double-page spreads
