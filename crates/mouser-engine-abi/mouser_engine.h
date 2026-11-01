/* Mouser custom rendering engine ABI, version 1. See docs/engines.md. */
#ifndef MOUSER_ENGINE_H
#define MOUSER_ENGINE_H
#include <stddef.h>
#include <stdint.h>

#define MOUSER_ENGINE_ABI 1
enum { MOUSER_SURFACE_WAYLAND = 1, MOUSER_SURFACE_XLIB = 2, MOUSER_SURFACE_XCB = 3,
       MOUSER_SURFACE_APPKIT = 4, MOUSER_SURFACE_WIN32 = 5,
       MOUSER_SURFACE_PIXELS = 6 /* window -> MouserPixels; used in the engine host */ };
enum { MOUSER_OP_RECT = 0, MOUSER_OP_GLYPH = 1, MOUSER_OP_IMAGE = 2 };
enum { MOUSER_DRAW_OK = 0, MOUSER_DRAW_AGAIN = 1 };

typedef struct { uint32_t kind; void *display; void *window; } MouserSurface;
typedef struct { uint32_t *data; uint32_t width, height, stride; } MouserPixels; /* 0x00RRGGBB */
typedef struct { int32_t x, y, w, h; uint32_t color; } MouserRect;
typedef struct { int32_t x, y; uint32_t w, h, stride, color; const uint8_t *coverage; uint64_t key; } MouserGlyph;
typedef struct { int32_t x, y; uint32_t w, h, img_w, img_h, src_x, src_y, src_w, src_h;
                 const uint8_t *rgba; uint64_t key; } MouserImage;
typedef struct { uint32_t kind, index; } MouserOp;
typedef struct {
    uint32_t width, height, clear;
    int32_t dirty_x, dirty_y, dirty_w, dirty_h;
    const MouserOp *ops; size_t op_count;
    const MouserRect *rects; size_t rect_count;
    const MouserGlyph *glyphs; size_t glyph_count;
    const MouserImage *images; size_t image_count;
} MouserFrame;

typedef struct {
    uint32_t abi;   /* MOUSER_ENGINE_ABI */
    uint32_t size;  /* sizeof(MouserEngineV1) */
    const char *name;
    void *(*create)(const MouserSurface *surface, uint32_t w, uint32_t h, char *err, size_t err_len);
    void (*resize)(void *engine, uint32_t w, uint32_t h);
    int (*draw)(void *engine, const MouserFrame *frame);
    void (*destroy)(void *engine);
} MouserEngineV1;

/* Exported by every engine library. */
const MouserEngineV1 *mouser_engine_v1(void);
#endif
