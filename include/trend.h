#ifndef TREND_H
#define TREND_H

#include <stddef.h>
#include <stdint.h>

#if defined(_WIN32)
#define TREND_API __declspec(dllimport)
#else
#define TREND_API
#endif

#ifdef __cplusplus
extern "C" {
#endif

typedef struct TrendColor {
    uint8_t red;
    uint8_t green;
    uint8_t blue;
    uint8_t kind;
} TrendColor;

typedef struct TrendCell {
    uint32_t glyph;
    TrendColor foreground;
    TrendColor background;
    uint8_t attributes;
    uint8_t reserved[3];
} TrendCell;

enum TrendFitMode {
    TREND_FIT_CONTAIN = 0,
    TREND_FIT_COVER = 1,
    TREND_FIT_STRETCH = 2
};

enum TrendResult {
    TREND_OK = 0,
    TREND_ERROR_NULL_POINTER = 1,
    TREND_ERROR_INVALID_FRAME = 2,
    TREND_ERROR_INVALID_VIEWPORT = 3,
    TREND_ERROR_OUTPUT_TOO_SMALL = 4,
    TREND_ERROR_RENDER = 5,
    TREND_ERROR_INVALID_OPTIONS = 6
};

enum TrendSamplingFilter {
    TREND_SAMPLE_NEAREST = 0,
    TREND_SAMPLE_BILINEAR = 1,
    TREND_SAMPLE_AREA = 2
};

enum TrendAlignment {
    TREND_ALIGN_START = 0,
    TREND_ALIGN_CENTER = 1,
    TREND_ALIGN_END = 2
};

typedef struct TrendRenderOptions {
    uint32_t struct_size;
    uint8_t sampling;
    uint8_t dither_strength;
    uint8_t horizontal_alignment;
    uint8_t vertical_alignment;
    uint16_t zoom_percent;
    uint16_t reserved;
    int32_t pan_x_cells;
    int32_t pan_y_cells;
} TrendRenderOptions;

TREND_API uint32_t trend_version(void);
TREND_API size_t trend_cell_size(void);

TREND_API int32_t trend_render_rgba8(
    const uint8_t *pixels,
    size_t pixels_len,
    uint32_t source_width,
    uint32_t source_height,
    size_t source_stride,
    uint16_t viewport_width,
    uint16_t viewport_height,
    uint8_t fit_mode,
    uint64_t generation,
    TrendCell *output_cells,
    size_t output_capacity
);

TREND_API int32_t trend_render_rgba8_ex(
    const uint8_t *pixels,
    size_t pixels_len,
    uint32_t source_width,
    uint32_t source_height,
    size_t source_stride,
    uint16_t viewport_width,
    uint16_t viewport_height,
    uint8_t fit_mode,
    uint64_t generation,
    const TrendRenderOptions *options,
    TrendCell *output_cells,
    size_t output_capacity
);

#ifdef __cplusplus
}
#endif

#endif