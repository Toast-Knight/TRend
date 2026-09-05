# Third-Party Notices

The default TRend core has no third-party package dependencies.
The complete all-feature Rust package and license snapshot is maintained in
[`DEPENDENCY_LICENSES.md`](DEPENDENCY_LICENSES.md). Package license files from
the resolved Cargo source are authoritative.

## trend-video

The optional Windows `trend-video` adapter uses generated Windows API bindings.
These packages are not linked into the default `trend` core artifact.

| Package | Version | Declared license |
| --- | --- | --- |
| windows | 0.62.2 | MIT OR Apache-2.0 |
| windows-collections | 0.3.2 | MIT OR Apache-2.0 |
| windows-core | 0.62.2 | MIT OR Apache-2.0 |
| windows-future | 0.3.2 | MIT OR Apache-2.0 |
| windows-implement | 0.60.2 | MIT OR Apache-2.0 |
| windows-interface | 0.59.3 | MIT OR Apache-2.0 |
| windows-link | 0.2.1 | MIT OR Apache-2.0 |
| windows-numerics | 0.3.1 | MIT OR Apache-2.0 |
| windows-result | 0.4.1 | MIT OR Apache-2.0 |
| windows-strings | 0.5.1 | MIT OR Apache-2.0 |
| windows-threading | 0.2.1 | MIT OR Apache-2.0 |
| proc-macro2 | 1.0.107 | MIT OR Apache-2.0 |
| quote | 1.0.47 | MIT OR Apache-2.0 |
| syn | 2.0.119 | MIT OR Apache-2.0 |
| unicode-ident | 1.0.24 | (MIT OR Apache-2.0) AND Unicode-3.0 |

The adapter dynamically uses Media Foundation and COM libraries supplied by
Windows. No FFmpeg or third-party codec binary is distributed.

The disabled-by-default Linux `linux-webm`, `linux-mkv`, and `linux-h264`
features additionally use:

| Package | Version | Declared license |
| --- | --- | --- |
| matroska-demuxer | 0.8.1 | Zlib OR MIT OR Apache-2.0 |
| libvpx-native-sys | 5.0.17 | MPL-2.0 |
| openh264 | 0.9.8 | BSD-2-Clause |
| openh264-sys2 | 0.9.8 | BSD-2-Clause |
| libloading | 0.8.9 | ISC |

`libvpx-native-sys` links to a separately installed system libvpx. Binary
distributors must also satisfy libvpx's upstream license and notice terms.
`linux-h264` dynamically loads a separately installed OpenH264 runtime. Binary
distributors must satisfy OpenH264's BSD license and any applicable codec patent
terms for their distribution channel.

## trend-animation

The optional `trend-animation` GIF adapter uses the following Rust packages.
These packages are not linked into the default `trend` core artifact.

| Package | Version | Declared license |
| --- | --- | --- |
| gif | 0.14.2 | MIT OR Apache-2.0 |
| weezl | 0.1.12 | MIT OR Apache-2.0 |

## trend-image

The optional `trend-image` PNG/JPEG adapter uses the following Rust packages.
These packages are not linked into the default `trend` core artifact.

| Package | Version | Declared license |
| --- | --- | --- |
| jpeg-decoder | 0.3.2 | MIT OR Apache-2.0 |
| jpeg-encoder (development only) | 0.6.1 | (MIT OR Apache-2.0) AND IJG |
| png | 0.18.1 | MIT OR Apache-2.0 |
| bitflags | 2.13.1 | MIT OR Apache-2.0 |
| crc32fast | 1.5.1 | MIT OR Apache-2.0 |
| cfg-if | 1.0.4 | MIT OR Apache-2.0 |
| fdeflate | 0.3.7 | MIT OR Apache-2.0 |
| flate2 | 1.1.10 | MIT OR Apache-2.0 |
| miniz_oxide | 0.8.9, 0.9.1 | MIT OR Zlib OR Apache-2.0 |
| adler2 | 2.0.1 | 0BSD OR MIT OR Apache-2.0 |
| simd-adler32 | 0.3.10 | MIT |

Complete license texts must accompany binary distributions that include the
optional adapter. The authoritative package versions and checksums are recorded
in `Cargo.lock`.

## Experimental WebP

The disabled-by-default `webp` feature is experimental and is not production
format support. The earlier `webp-spike` feature remains an alias. Builds that
explicitly enable either name add these packages:

| Package | Version | Declared license |
| --- | --- | --- |
| image-webp | 0.2.4 | MIT OR Apache-2.0 |
| byteorder-lite | 0.1.0 | Unlicense OR MIT |
| quick-error | 2.0.1 | MIT OR Apache-2.0 |

See [APIS.md](APIS.md) and the adapter READMEs for current experimental-format
status and bounds.

## Linux Terminal Viewer

The Linux interactive test viewer uses `crossterm` 0.29.0 under the MIT
license. Its platform and synchronization dependencies are included in the
complete dependency inventory linked above.