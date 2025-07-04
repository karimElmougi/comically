# comically

comically fast manga/comic optimizer for e-readers

![comically splash screen](assets/goku-splash-original.jpg)

## what's this?

tired of manga looking terrible on your kindle? waiting forever for conversions?

comically optimizes manga/comics specifically for e-ink displays. live preview in the terminal shows exactly how it'll look on your device.

**built for e-ink:**
- deep blacks and proper contrast (not washed out lcd optimization)
- perfectly sized for your device (clipping excess margin, smaller files)
- tiny files that load instantly 

**actually fast:**
> with spread splitting and rotating enabled
- 23 volumes of dorohedoro (4647 images, 2.5gb) 
   - to epub → 45 seconds
   - to awz3/mobi -> 105 seconds
- 9 volumes of Alice in Borderland (3064 images, 4.5gb)
   - to epub -> 55 seconds
   - to awz3/mobi -> 77 seconds

**features:**
- see image previews in your terminal while you tweak settings
- batch process entire series
- smart page splitting for double spreads
- auto contrast 
- remembers your settings for next time

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
