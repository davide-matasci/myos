/* cairo-pango-smoke: the cairo-pango package's boot test, against mlibc.
 *
 * fribidi reorders a mixed left-to-right and Hebrew line; harfbuzz shapes
 * a word with DejaVu Sans Mono (the x11-fonts package); pango lays out a
 * line in "DejaVu Sans", which fontconfig finds (x11-xft's fonts.conf),
 * and cairo draws it on an image surface, saved as a PNG that gdk-pixbuf
 * loads back, and re-saved as a JPEG that it loads from memory. Then, on
 * Xfbdev (display :0), cairo's Xlib surface fills a rectangle that
 * XGetImage reads back, and libepoxy is asked whether there is GLX (no:
 * there is no GL library to load). Prints [ OK ] cairo-pango.
 */
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <unistd.h>

#include <cairo-xlib.h>
#include <cairo.h>
#include <epoxy/glx.h>
#include <fribidi.h>
#include <gdk-pixbuf/gdk-pixbuf.h>
#include <harfbuzz/hb.h>
#include <pango/pangocairo.h>
#include <X11/Xlib.h>
#include <X11/Xutil.h>

#define W 240
#define H 60
#define PNG "/tmp/cairo-pango.png"
#define MONO "/lib/fonts/DejaVuSansMono.ttf"

static int fail(const char *what) {
    printf("[ FAIL ] cairo-pango %s\n", what);
    return 1;
}

/* "ab " then Hebrew alef, bet, gimel: shown left to right, the Hebrew
 * reversed. */
static int bidi(void) {
    FriBidiChar logical[] = {'a', 'b', ' ', 0x5d0, 0x5d1, 0x5d2}, visual[6];
    FriBidiParType base = FRIBIDI_PAR_LTR;
    if (!fribidi_log2vis(logical, 6, &base, visual, NULL, NULL, NULL)) {
        return fail("fribidi_log2vis");
    }
    if (visual[0] != 'a' || visual[3] != 0x5d2 || visual[5] != 0x5d0) {
        return fail("fribidi order");
    }
    return 0;
}

/* harfbuzz: "myos" in DejaVu Sans Mono is four glyphs, none missing, of
 * one advance each (a monospaced font). */
static int shape(void) {
    hb_blob_t *blob = hb_blob_create_from_file_or_fail(MONO);
    if (!blob) {
        return fail("no " MONO);
    }
    hb_face_t *face = hb_face_create(blob, 0);
    hb_font_t *font = hb_font_create(face);
    hb_buffer_t *buf = hb_buffer_create();
    hb_buffer_add_utf8(buf, "myos", -1, 0, -1);
    hb_buffer_guess_segment_properties(buf);
    hb_shape(font, buf, NULL, 0);
    unsigned n;
    hb_glyph_info_t *info = hb_buffer_get_glyph_infos(buf, &n);
    hb_glyph_position_t *pos = hb_buffer_get_glyph_positions(buf, NULL);
    int ok = n == 4;
    for (unsigned i = 0; ok && i < n; i++) {
        ok = info[i].codepoint != 0 && pos[i].x_advance == pos[0].x_advance && pos[i].x_advance > 0;
    }
    hb_buffer_destroy(buf);
    hb_font_destroy(font);
    hb_face_destroy(face);
    hb_blob_destroy(blob);
    return ok ? 0 : fail("harfbuzz glyphs");
}

/* pango and cairo: black text on white; returns its dark pixels. */
static int draw_text(cairo_surface_t *s, int *dark, int *tw) {
    cairo_t *cr = cairo_create(s);
    cairo_set_source_rgb(cr, 1, 1, 1);
    cairo_paint(cr);
    PangoLayout *layout = pango_cairo_create_layout(cr);
    PangoFontDescription *desc = pango_font_description_from_string("DejaVu Sans 20");
    pango_layout_set_font_description(layout, desc);
    pango_layout_set_text(layout, "myos pango", -1);
    int th;
    pango_layout_get_pixel_size(layout, tw, &th);
    cairo_set_source_rgb(cr, 0, 0, 0);
    cairo_move_to(cr, 4, 4);
    pango_cairo_show_layout(cr, layout);
    cairo_surface_flush(s);
    const char *family = pango_font_description_get_family(
        pango_font_describe(pango_context_load_font(pango_layout_get_context(layout), desc)));
    int ok = family && strcmp(family, "DejaVu Sans") == 0;
    pango_font_description_free(desc);
    g_object_unref(layout);
    cairo_destroy(cr);
    if (!ok) {
        printf("[ FAIL ] cairo-pango font is %s, not DejaVu Sans\n", family ? family : "none");
        return 1;
    }
    unsigned char *px = cairo_image_surface_get_data(s);
    int stride = cairo_image_surface_get_stride(s);
    *dark = 0;
    for (int y = 0; y < H; y++) {
        for (int x = 0; x < W; x++) {
            *dark += (((uint32_t *)(px + y * stride))[x] & 0xff) < 0x80;
        }
    }
    return 0;
}

/* gdk-pixbuf: the PNG back from the file, then as a JPEG through memory. */
static int pixbufs(void) {
    GError *err = NULL;
    GdkPixbuf *png = gdk_pixbuf_new_from_file(PNG, &err);
    if (!png) {
        printf("[ FAIL ] cairo-pango gdk-pixbuf png: %s\n", err->message);
        return 1;
    }
    if (gdk_pixbuf_get_width(png) != W || gdk_pixbuf_get_height(png) != H) {
        return fail("gdk-pixbuf png size");
    }
    gchar *jpeg;
    gsize size;
    if (!gdk_pixbuf_save_to_buffer(png, &jpeg, &size, "jpeg", &err, "quality", "90", NULL)) {
        printf("[ FAIL ] cairo-pango gdk-pixbuf jpeg save: %s\n", err->message);
        return 1;
    }
    GInputStream *in = g_memory_input_stream_new_from_data(jpeg, size, g_free);
    GdkPixbuf *back = gdk_pixbuf_new_from_stream(in, NULL, &err);
    if (!back || gdk_pixbuf_get_width(back) != W || gdk_pixbuf_get_has_alpha(back)) {
        printf("[ FAIL ] cairo-pango gdk-pixbuf jpeg: %s\n", err ? err->message : "size");
        return 1;
    }
    g_object_unref(back);
    g_object_unref(in);
    g_object_unref(png);
    return 0;
}

/* Whether the server listens on display :0's socket yet. */
static int server_up(void) {
    struct sockaddr_un a = {.sun_family = AF_UNIX, .sun_path = "/tmp/.X11-unix/X0"};
    int fd = socket(AF_UNIX, SOCK_STREAM, 0);
    int up = fd >= 0 && connect(fd, (struct sockaddr *)&a, sizeof a) == 0;
    if (fd >= 0) {
        close(fd);
    }
    return up;
}

/* cairo's Xlib surface on a window, read back with XGetImage; libepoxy's
 * GLX check. */
static int xlib(int *glx) {
    for (int i = 0; i < 60 && !server_up(); i++) {
        sleep(1);
    }
    Display *dpy = XOpenDisplay(":0");
    if (!dpy) {
        return fail("XOpenDisplay");
    }
    int scr = DefaultScreen(dpy);
    XSetWindowAttributes wa = {.override_redirect = True, .background_pixel = BlackPixel(dpy, scr)};
    Window win = XCreateWindow(dpy, RootWindow(dpy, scr), 0, 0, 200, 100, 0, CopyFromParent, InputOutput,
                               CopyFromParent, CWOverrideRedirect | CWBackPixel, &wa);
    XMapWindow(dpy, win);
    XSync(dpy, False);
    cairo_surface_t *s = cairo_xlib_surface_create(dpy, win, DefaultVisual(dpy, scr), 200, 100);
    cairo_t *cr = cairo_create(s);
    cairo_set_source_rgb(cr, 0, 0, 1);
    cairo_rectangle(cr, 20, 20, 40, 40);
    cairo_fill(cr);
    cairo_destroy(cr);
    cairo_surface_flush(s);
    XSync(dpy, False);
    XImage *img = XGetImage(dpy, win, 30, 30, 1, 1, AllPlanes, ZPixmap);
    unsigned long p = img ? XGetPixel(img, 0, 0) & 0xffffff : 0xdeadbeef;
    if (img) {
        XDestroyImage(img);
    }
    if (p != 0x0000ff) {
        printf("[ FAIL ] cairo-pango xlib pixel %06lx\n", p);
        return 1;
    }
    *glx = epoxy_has_glx(dpy);
    cairo_surface_destroy(s);
    XCloseDisplay(dpy);
    return 0;
}

int main(void) {
    int dark, tw, glx;
    if (bidi() || shape()) {
        return 1;
    }
    cairo_surface_t *s = cairo_image_surface_create(CAIRO_FORMAT_ARGB32, W, H);
    if (draw_text(s, &dark, &tw)) {
        return 1;
    }
    if (dark < 100 || tw < 80) {
        printf("[ FAIL ] cairo-pango text: %d dark pixels, %d wide\n", dark, tw);
        return 1;
    }
    if (cairo_surface_write_to_png(s, PNG) != CAIRO_STATUS_SUCCESS) {
        return fail("cairo png");
    }
    cairo_surface_destroy(s);
    if (pixbufs() || xlib(&glx)) {
        return 1;
    }
    unlink(PNG);
    printf("[ OK ] cairo-pango text %d px wide, %d dark pixels, GLX %s\n", tw, dark, glx ? "yes" : "no");
    return 0;
}
