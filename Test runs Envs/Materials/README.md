# Test Fixture Policy

This directory contains compact synthetic media used by TRend's automated and
interactive format tests. The fixtures exercise dimensions, color, alpha,
timing, audio-stream metadata, seeking, malformed input, and decoder bounds.

JPEG fixtures are regenerated with `../generate_jpeg_fixtures.ps1`. WebP
fixtures are regenerated with `../generate_webp_spike_fixtures.py`. The
format-specific subdirectories contain small codec/container regression cases.
Files named `corrupt` are intentionally malformed and must not be used as normal
sample media.

Do not add downloaded, personal, or third-party media without documenting its
source and redistribution license. Prefer deterministic synthetic fixtures and
keep each file as small as its regression purpose permits.

These project-created fixtures are distributed under the repository's Apache
License 2.0 unless a fixture is accompanied by a more specific notice.
