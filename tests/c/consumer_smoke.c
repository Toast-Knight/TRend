/*
 * C consumer smoke test for include/trend.h (C ABI v1).
 *
 * Checks at compile time that every struct in the header has the size and
 * field offsets the Rust side uses, and at run time that both render
 * functions work, agree with each other, and report each documented error.
 *
 * Build and run against the release cdylib (see scripts/c-consumer-smoke.sh):
 *   cc -std=c11 -Wall -Wextra -Werror -Iinclude tests/c/consumer_smoke.c \
 *      -Ltarget/release -ltrend -o consumer_smoke && ./consumer_smoke
 */
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>

#include "trend.h"

_Static_assert(sizeof(TrendColor) == 4, "TrendColor is 4 bytes");
_Static_assert(sizeof(TrendCell) == 16, "TrendCell is 16 bytes");
_Static_assert(offsetof(TrendCell, glyph) == 0, "glyph offset");
_Static_assert(offsetof(TrendCell, foreground) == 4, "foreground offset");
_Static_assert(offsetof(TrendCell, background) == 8, "background offset");
_Static_assert(offsetof(TrendCell, attributes) == 12, "attributes offset");
_Static_assert(offsetof(TrendCell, reserved) == 13, "reserved offset");
_Static_assert(sizeof(TrendRenderOptions) == 20, "TrendRenderOptions is 20 bytes");
_Static_assert(offsetof(TrendRenderOptions, zoom_percent) == 8, "zoom offset");
_Static_assert(offsetof(TrendRenderOptions, pan_x_cells) == 12, "pan x offset");
_Static_assert(offsetof(TrendRenderOptions, pan_y_cells) == 16, "pan y offset");

static int failures = 0;

#define CHECK(condition)                                                   \
    do {                                                                   \
        if (!(condition)) {                                                \
            fprintf(stderr, "FAIL %s:%d: %s\n", __FILE__, __LINE__, #condition); \
            failures++;                                                    \
        }                                                                  \
    } while (0)

static TrendRenderOptions default_options(void) {
    TrendRenderOptions options;
    memset(&options, 0, sizeof options);
    options.struct_size = (uint32_t)sizeof options;
    options.sampling = TREND_SAMPLE_NEAREST;
    options.horizontal_alignment = TREND_ALIGN_CENTER;
    options.vertical_alignment = TREND_ALIGN_CENTER;
    options.zoom_percent = 100;
    return options;
}

int main(void) {
    CHECK(trend_version() == 0x00010000u);
    CHECK(trend_cell_size() == sizeof(TrendCell));

    /* A 6x4 gradient with padded rows (stride 28 bytes, not 24). */
    enum { W = 6, H = 4, STRIDE = W * 4 + 4 };
    uint8_t pixels[STRIDE * H];
    memset(pixels, 0xA5, sizeof pixels);
    for (int y = 0; y < H; y++) {
        for (int x = 0; x < W; x++) {
            uint8_t *p = &pixels[y * STRIDE + x * 4];
            p[0] = (uint8_t)(x * 40);
            p[1] = (uint8_t)(y * 60);
            p[2] = 200;
            p[3] = 255;
        }
    }

    TrendCell plain[6 * 2];
    TrendCell extended[6 * 2];
    int32_t result = trend_render_rgba8(pixels, sizeof pixels, W, H, STRIDE, 6, 2,
                                        TREND_FIT_STRETCH, 7, plain, 12);
    CHECK(result == TREND_OK);
    TrendRenderOptions options = default_options();
    result = trend_render_rgba8_ex(pixels, sizeof pixels, W, H, STRIDE, 6, 2,
                                   TREND_FIT_STRETCH, 7, &options, extended, 12);
    CHECK(result == TREND_OK);
    CHECK(memcmp(plain, extended, sizeof plain) == 0);

    /* Stretching 6x4 onto 6x2 half-block cells maps pixels one to one: the
     * first cell shows pixel (0,0) over pixel (0,1). */
    CHECK(plain[0].glyph == 0x2580); /* upper half block */
    CHECK(plain[0].foreground.red == 0 && plain[0].foreground.green == 0);
    CHECK(plain[0].foreground.blue == 200 && plain[0].foreground.kind == 1);
    CHECK(plain[0].background.green == 60 && plain[0].background.kind == 1);
    CHECK(plain[5].foreground.red == 200);
    CHECK(plain[6].foreground.green == 120 && plain[6].background.green == 180);
    for (int i = 0; i < 12; i++) {
        CHECK(plain[i].attributes == 0);
    }

    /* Documented errors. */
    CHECK(trend_render_rgba8(NULL, 0, W, H, STRIDE, 6, 2, 0, 0, plain, 12) ==
          TREND_ERROR_NULL_POINTER);
    CHECK(trend_render_rgba8(pixels, sizeof pixels, W, H, STRIDE, 6, 2, 0, 0, NULL, 12) ==
          TREND_ERROR_NULL_POINTER);
    CHECK(trend_render_rgba8(pixels, sizeof pixels, W, H, STRIDE, 0, 2, 0, 0, plain, 12) ==
          TREND_ERROR_INVALID_VIEWPORT);
    CHECK(trend_render_rgba8(pixels, sizeof pixels, W, H, STRIDE, 6, 2, 9, 0, plain, 12) ==
          TREND_ERROR_INVALID_VIEWPORT);
    CHECK(trend_render_rgba8(pixels, sizeof pixels, W, H, STRIDE, 6, 2, 0, 0, plain, 11) ==
          TREND_ERROR_OUTPUT_TOO_SMALL);
    /* The last row needs no padding, so this is the shortest valid buffer... */
    const size_t shortest = (size_t)(H - 1) * STRIDE + W * 4;
    CHECK(trend_render_rgba8(pixels, shortest, W, H, STRIDE, 6, 2, 0, 0, plain, 12) == TREND_OK);
    /* ...and one byte less is rejected. */
    CHECK(trend_render_rgba8(pixels, shortest - 1, W, H, STRIDE, 6, 2, 0, 0, plain, 12) ==
          TREND_ERROR_INVALID_FRAME);
    CHECK(trend_render_rgba8(pixels, sizeof pixels, W, H, W * 4 - 1, 6, 2, 0, 0, plain, 12) ==
          TREND_ERROR_INVALID_FRAME);
    CHECK(trend_render_rgba8_ex(pixels, sizeof pixels, W, H, STRIDE, 6, 2, 0, 0, NULL, plain,
                                12) == TREND_ERROR_NULL_POINTER);
    options = default_options();
    options.struct_size = 16;
    CHECK(trend_render_rgba8_ex(pixels, sizeof pixels, W, H, STRIDE, 6, 2, 0, 0, &options,
                                plain, 12) == TREND_ERROR_INVALID_OPTIONS);
    options = default_options();
    options.sampling = 3;
    CHECK(trend_render_rgba8_ex(pixels, sizeof pixels, W, H, STRIDE, 6, 2, 0, 0, &options,
                                plain, 12) == TREND_ERROR_INVALID_OPTIONS);
    options = default_options();
    options.zoom_percent = 0;
    CHECK(trend_render_rgba8_ex(pixels, sizeof pixels, W, H, STRIDE, 6, 2, 0, 0, &options,
                                plain, 12) == TREND_ERROR_INVALID_OPTIONS);

    if (failures == 0) {
        printf("C consumer smoke test: all checks passed\n");
        return 0;
    }
    fprintf(stderr, "C consumer smoke test: %d failure(s)\n", failures);
    return 1;
}
