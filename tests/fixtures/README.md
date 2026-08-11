# Test fixtures

Two-second 440Hz synthetic tones, generated with ffmpeg (see the plan). They
exercise the decode paths — stereo→mono, 44.1k/48k→16k — without shipping real
audio. `ffmpeg` is a developer-only dependency; the app does not use it.

Regenerate with:

```bash
ffmpeg -y -f lavfi -i "sine=frequency=440:duration=2" -ar 44100 -ac 2 tests/fixtures/tone.mp3
ffmpeg -y -f lavfi -i "sine=frequency=440:duration=2" -ar 48000 -ac 1 tests/fixtures/tone.wav
ffmpeg -y -f lavfi -i "sine=frequency=440:duration=2" -ar 44100 -ac 2 tests/fixtures/tone.m4a
ffmpeg -y -f lavfi -i "sine=frequency=440:duration=2" -c:a libopus tests/fixtures/tone.opus
```

`tone.opus` additionally covers the Opus path, which routes through
`symphonia-adapter-libopus` rather than a native symphonia codec.
