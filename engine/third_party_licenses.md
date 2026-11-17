# Third-party code used by the engine

Nothing listed here is vendored into git. CMake fetches the SDKs at configure
time (`FetchContent`, see `engine/CMakeLists.txt`) into the build tree
(`engine/build*/_deps/`, or `target/dstudio-third-party/` for cargo builds),
both of which are git-ignored. Offline builds can point CMake at local
checkouts with `-DFETCHCONTENT_SOURCE_DIR_DSTUDIO_CLAP=<dir>` and
`-DFETCHCONTENT_SOURCE_DIR_DSTUDIO_VST3SDK=<dir>`.

| Component | Source | Pinned tag (commit) | License | What we use |
|---|---|---|---|---|
| CLAP | https://github.com/free-audio/clap | `1.2.10` (`195b42a004144fab0b3cf95e9c067187d15365b7`, 2026-07-13) | MIT | Headers only (`include/clap/…`): hosting, scanner, test plugins |
| VST 3 SDK | https://github.com/steinbergmedia/vst3sdk | `v3.8.1_build_84` (`3cdf9ca5d1f5b1b21e0a86832aa4abe55607bd96`, 2026-08-11); submodules `pluginterfaces` (`4f547e8e`), `public.sdk` (`586dc5e6`) | MIT (since VST 3.8) | `pluginterfaces` headers + IID definitions (`funknown.cpp`, `coreiids.cpp`, `vstinitiids.cpp`, `commoniids.cpp`); host helpers from `public.sdk/source/vst/hosting` (`hostclasses.cpp`, `pluginterfacesupport.cpp`) with `vst/utility/stringconvert.cpp` and `common/commonstringconvert.cpp` |
| signalsmith-stretch | https://github.com/Signalsmith-Audio/signalsmith-stretch | `1.4.0` (`a670068d9aeb64913331d5cc29337b19a457a7df`, 2026-09-25) | MIT | Header-only time stretcher (`signalsmith-stretch.h`), wrapped by `src/warp.cpp` for warped audio clips |
| signalsmith-linear | https://github.com/Signalsmith-Audio/linear | `0.6.6` (`7bb1c8f36794d6980080ce817c91f8da8fe56a66`, 2026-10-21) | MIT | Header-only FFT / STFT used by signalsmith-stretch (vDSP backend on macOS via Accelerate) |
| dr_libs (dr_mp3, dr_flac, dr_wav) | https://github.com/mackron/dr_libs | commit `f12e197fd462e4c22873fbd70d11b1d2f2c1dc3c` (2026-11-11): dr_mp3 0.7.5, dr_flac 0.13.5, dr_wav 0.14.7 | Public domain (Unlicense) or MIT No Attribution; we use it under MIT-0. dr_mp3 embeds minimp3 (CC0) | Single-file decoders for MP3, FLAC and AIFF / AIFC import (`src/decode.cpp`, implementations in `src/decoders_impl.c`); dr_wav is used for AIFF only, WAV keeps our own reader |
| stb_vorbis (stb) | https://github.com/nothings/stb | commit `9600522de53973336fa033a289858a4354cbb04c` (2026-11-12), stb_vorbis 1.24 | MIT or public domain (Unlicense); we use it under MIT | Ogg Vorbis import (`src/decode.cpp`) |
| Audio Units, CoreAudioKit, AppKit | Apple system frameworks | macOS SDK | Apple SDK terms | AU hosting (`AudioToolbox`), `AUGenericView` / `AUCocoaUIBase` editors, NSWindow |

Notes:

* Only the `pluginterfaces` and `public.sdk` submodules of the VST 3 SDK are
  checked out (no `base`, `vstgui4`, `tutorials`, `doc`). The ASIO SDK is a
  separate product (GPLv3 / proprietary) and is **not** used.
* "VST" is a trademark of Steinberg Media Technologies GmbH. The MIT license
  covers the SDK code; use of the VST logo/trademark is governed separately by
  Steinberg's usage guidelines (`VST3_Usage_Guidelines.pdf` in the SDK). We
  ship no logo.
* The test plugins in `engine/test_plugins/` are original code written for
  this project against the SDK interfaces; they are built into the build tree
  only and are not distributed.

## CLAP — MIT License

```
MIT License

Copyright (c) 2021 Alexandre BIQUE

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## VST 3 SDK — MIT License

(Text of `LICENSE.txt` at `v3.8.1_build_84`; the `pluginterfaces` and
`public.sdk` submodules carry the same license.)

```
MIT License

Copyright (c) 2026, Steinberg Media Technologies GmbH

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## signalsmith-stretch — MIT License

```
MIT License

Copyright (c) 2022 Geraint Luff / Signalsmith Audio Ltd.

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## signalsmith-linear — MIT License

```
MIT License

Copyright (c) 2025 Signalsmith Audio

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## dr_libs (dr_mp3, dr_flac, dr_wav): MIT No Attribution

(Alternative 2 of each file's dual licence; the other is the Unlicense.)

```
Copyright 2023 David Reid

Permission is hereby granted, free of charge, to any person obtaining a copy of
this software and associated documentation files (the "Software"), to deal in
the Software without restriction, including without limitation the rights to
use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies
of the Software, and to permit persons to whom the Software is furnished to do
so.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

dr_mp3 is based on minimp3 (https://github.com/lieff/minimp3), dedicated to the
public domain under CC0 1.0 by its authors.

## stb_vorbis: MIT License

(Alternative A of the file's dual licence; the other is the Unlicense.)

```
Copyright (c) 2017 Sean Barrett

Permission is hereby granted, free of charge, to any person obtaining a copy of
this software and associated documentation files (the "Software"), to deal in
the Software without restriction, including without limitation the rights to
use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies
of the Software, and to permit persons to whom the Software is furnished to do
so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## App: LAME (MP3 export)

The app (not the engine) links LAME for MP3 export, through the Rust crates
`mp3lame-encoder` 0.3.0 and `mp3lame-sys` 0.2.0 (crates.io), which compile
LAME's `libmp3lame` from the sources shipped in the `mp3lame-sys` crate
(`lame-4.0`) into the app binary. Used in `crates/dstudio-app/src/mp3.rs`.

| Component | Source | Version | License |
|---|---|---|---|
| LAME (`libmp3lame`) | https://lame.sourceforge.io/ via https://github.com/DoumanAsh/mp3lame-sys | as bundled in `mp3lame-sys` 0.2.0 | GNU Library (Lesser) General Public License, version 2 or later |
| `mp3lame-sys`, `mp3lame-encoder` (Rust bindings) | https://github.com/DoumanAsh/mp3lame-encoder | 0.2.0, 0.3.0 | LGPL-3.0 |

The LGPL asks that people receiving the app can relink it against a
modified LAME. Device's complete source (AGPL-3.0-or-later) is public, so a
modified LAME can be built into it with `cargo` (for example with a
`[patch.crates-io]` entry for `mp3lame-sys`). The license texts ship in the
crates (`COPYING` in `lame-4.0`, `LICENSE` in each crate).
