# comically

blazing fast manga/comic optimizer. IMAGES IN YOUR TERMINAL.

![comically splash screen](assets/goku-splash-original.jpg)

## what's this?

tired of washed out manga on your kindle? comics with huge margins? waiting forever for conversions?

comically is a futuristic tui that shows live previews RIGHT IN THE TERMINAL. watch your adjustments in real-time.

**actually fast:**
- 23 volumes of dorohedoro (4647 images, 2.5gb) → epub in 45 seconds
- that's 100+ images per second
- with full processing: splitting, rotation, optimization

**fixes the annoying stuff:**
- washed out blacks → deep contrast that looks good on e-ink
- wasted margins → fullscreen pages
- wrong resolution → uses all 2480 pixels on your scribe
- broken spreads → perfect alignment
- huge files → 312mb → 28mb (no quality loss)

**features:**
- see your comics in the terminal (seriously)
- live preview as you adjust settings
- batch process entire series
- remembers your settings

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

## pro tips

- gamma 1.8 = instant kindle optimization  
- most scans need brightness -15
- scribe users: you're using all 2480 pixels now
