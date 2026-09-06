# TRend Linux x64 package

```text
bin/       viewer and bridge executables
include/   C header
lib/       libtrend.so
samples/   PNG and GIF test media
docs/      bridge protocol
```

Run:

```sh
./bin/trend-test-viewer --help
./bin/trend-test-viewer ./samples/ColorTest.png
./bin/trend-test-viewer ./samples/ColorTest.gif
./bin/trend-test-viewer --probe 20x10 2 ./samples/ColorTest.png
```

The all-feature Linux build requires system libvpx while building and running.
The H.264 MKV feature dynamically loads a compatible system OpenH264 library.
MP4, AVI, and MOV decoding are not claimed on Linux.