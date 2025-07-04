# comically

comically fast manga/comic optimizer for e-readers

![comically splash screen](assets/goku-splash-original.jpg)

## what's this?

tired of manga looking terrible on your kindle? waiting forever for conversions?

comically optimizes manga/comics specifically for e-ink displays. processes 100+ pages per second. live image preview shows exactly how it'll look on your device

![preview](assets/preview.png)

**built for e-ink:**
- deep blacks and proper contrast
- perfectly sized for your device 
- smaller files so you can read more

**actually fast:**
> tested with spread splitting & rotation enabled on kindle pw 11 (1236x1648)

| series | volumes | pages | size | epub | awz3/mobi |
|--------|---------|-------|------|------|-----------|
| dorohedoro | 23 | 4,647 | 2.5gb | 45s | 105s |
| alice in borderland | 9 | 3,064 | 4.5gb | 55s | 77s |
| naruto | 72 | 12,849 | 17.5gb | 240s | 334s |

**features:**
- see image previews in your terminal while you tweak settings
- batch process entire series
- smart page splitting for double spreads
- auto contrast 

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

