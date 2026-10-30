# Recording a demo

Scripts for producing the GIF and screenshot that go in the README. X11 only, and everything they need is usually already installed: `ffmpeg`, `xdotool`, and ImageMagick (`import`).

| Script | What it does |
|---|---|
| `demo-session.sh` | Types a scripted tour into a running Winter window |
| `record.sh` | Records a window you click on, and encodes a GIF |
| `shot.sh` | Grabs a single PNG of a window you click on |

## The flow

```bash
# 1. Launch Winter itself (the GUI, not the headless path).
cargo run --release -p winter-term

# 2. In a second terminal, start the recorder, then click the Winter window.
./scripts/demo/record.sh 20 docs/demo.gif

# 3. In the Winter window, run the tour.
./scripts/demo/demo-session.sh
```

For a still instead:

```bash
./scripts/demo/shot.sh docs/screenshot.png
```

## What the tour shows

`demo-session.sh` types each command out one character at a time rather than dumping it, because a demo that scrolls faster than a viewer can read shows nothing.

It walks the three things that actually distinguish Winter:

1. Ordinary output still looks ordinary.
2. An SVG and a markdown table arriving as **blocks**, rendered inline rather than printed.
3. The same command piped elsewhere, still emitting its `text/plain` fallback.

Then it pauses, so you can press `Esc` and drive Normal mode by hand. The modal layer is the part no script fakes convincingly, and a still frame of it just looks like a terminal with a status bar, so it needs to be live and it needs motion.

## Tuning

| Variable | Default | Effect |
|---|---|---|
| `DEMO_SPEED` | `0.045` | Seconds per typed character |
| `DEMO_PAUSE` | `1.4` | Seconds to hold after each command |
| `FPS` | `12` | Recording frame rate; 12 is plenty for a terminal |
| `GIF_WIDTH` | `900` | Output width, which is roughly how wide GitHub renders a README |

```bash
FPS=10 GIF_WIDTH=800 ./scripts/demo/record.sh 15 docs/demo.gif
```

Aim for 10 to 15 seconds. `record.sh` warns if the result goes over 5MB; if it does, drop `FPS` or `GIF_WIDTH` before shortening the clip, since losing content costs more than losing frames.

## Why two-pass encoding

`record.sh` captures to H.264 first, then generates a palette per clip (`palettegen=stats_mode=diff`) and applies it with Bayer dithering. ffmpeg's default 256-colour quantiser turns antialiased terminal text to mud, and a single-pass GIF looks noticeably worse for the same file size.

## Suggested README layout

Lead with the static PNG of a rich block sitting next to normal shell output: it loads instantly and makes the whole pitch in one frame. Put the GIF below it for the vim navigation.

## Caveat

These have not been run end to end against a live Winter window. The ffmpeg pipeline is verified, and both scripts parse clean, but the tour's output depends on `clients/client.sh svg` and `markdown` behaving as its usage text documents. If a block does not render, run that `client.sh` line by hand first to tell whether the problem is the client or the recorder.
