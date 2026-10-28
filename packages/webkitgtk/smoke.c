/* webkitgtk-smoke: the webkitgtk package's boot test, against mlibc; also
 * WebKit's web and network processes (one binary: the patch's
 * MultiCall.cpp runs WebKitWebProcess and WebKitNetworkProcess, and this
 * main is the rest).
 *
 * On Xfbdev (display :0), a GTK window holds a web view, which loads URL
 * (the first argument; the launcher's test page, src/test_www.rs, by
 * default). Once WebKit says the load finished and the page's title is
 * there: its script ran (asked through JavaScript), and its blue box is
 * on the screen (read back from the window). Prints [ OK ] webkitgtk; a
 * wrong title shows as the lines before the timeout.
 */
#include <stdio.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <unistd.h>

#include <gtk/gtk.h>
#include <webkit2/webkit2.h>

#define URL "http://10.0.2.2:8766/hello.html"
#define TITLE "myos test page"
#define SCRIPT_TEXT "script ran 42"

static GtkWidget *view;
static int rc = 1, tries, finished;

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

static void done(const char *failure) {
    if (failure) {
        printf("[ FAIL ] webkitgtk %s\n", failure);
    } else {
        printf("[ OK ] webkitgtk title, script, pixels\n");
        rc = 0;
    }
    gtk_main_quit();
}

/* Whether the box's middle is blue on the screen. */
static int box_drawn(void) {
    GdkPixbuf *pb = gdk_pixbuf_get_from_window(gtk_widget_get_window(view), 100, 50, 1, 1);
    if (!pb) {
        return 0;
    }
    const guchar *p = gdk_pixbuf_read_pixels(pb);
    int blue = p[0] < 0x40 && p[1] < 0x40 && p[2] > 0xc0;
    g_object_unref(pb);
    return blue;
}

/* The page reaches the window a frame after it loaded: look a few times. */
static gboolean look(gpointer data) {
    (void)data;
    if (box_drawn()) {
        done(NULL);
        return G_SOURCE_REMOVE;
    }
    if (++tries == 300) {
        done("box not drawn");
        return G_SOURCE_REMOVE;
    }
    return G_SOURCE_CONTINUE;
}

static void script_done(GObject *object, GAsyncResult *result, gpointer data) {
    (void)data;
    GError *err = NULL;
    JSCValue *value = webkit_web_view_evaluate_javascript_finish(WEBKIT_WEB_VIEW(object), result, &err);
    if (!value) {
        printf("[ FAIL ] webkitgtk script: %s\n", err->message);
        done("script");
        return;
    }
    char *text = jsc_value_to_string(value);
    if (strcmp(text, SCRIPT_TEXT) != 0) {
        printf("[ FAIL ] webkitgtk the page's script wrote \"%s\"\n", text);
        done("script");
    } else {
        g_timeout_add(100, look, NULL);
    }
    g_free(text);
    g_object_unref(value);
}

/* Once the load finished and the title came (WebKit may tell it after the
 * load), ask for the script's work. */
static void loaded(WebKitWebView *v) {
    static int asked;
    const char *title = webkit_web_view_get_title(v);
    if (asked || !finished || !title || strcmp(title, TITLE) != 0) {
        return;
    }
    asked = 1;
    const char *js = "document.getElementById('js').textContent";
    webkit_web_view_evaluate_javascript(v, js, -1, NULL, NULL, NULL, script_done, NULL);
}

static void load_changed(WebKitWebView *v, WebKitLoadEvent event, gpointer data) {
    static const char *const names[] = {"started", "redirected", "committed", "finished"};
    (void)data;
    printf("webkitgtk: load %s\n", names[event]);
    finished = event == WEBKIT_LOAD_FINISHED;
    loaded(v);
}

static void title_changed(GObject *v, GParamSpec *spec, gpointer data) {
    (void)spec;
    (void)data;
    const char *title = webkit_web_view_get_title(WEBKIT_WEB_VIEW(v));
    printf("webkitgtk: title \"%s\"\n", title ? title : "");
    loaded(WEBKIT_WEB_VIEW(v));
}

static gboolean load_failed(WebKitWebView *v, WebKitLoadEvent event, char *uri, GError *err, gpointer data) {
    (void)v;
    (void)event;
    (void)data;
    printf("[ FAIL ] webkitgtk load %s: %s\n", uri, err->message);
    return FALSE;
}

static void web_process_terminated(WebKitWebView *v, WebKitWebProcessTerminationReason reason, gpointer data) {
    (void)v;
    (void)data;
    printf("[ FAIL ] webkitgtk web process ended: %s\n",
           reason == WEBKIT_WEB_PROCESS_CRASHED ? "crashed" : "out of memory or stopped");
    done("web process");
}

static gboolean timeout(gpointer data) {
    (void)data;
    done("timed out");
    return G_SOURCE_REMOVE;
}

int main(int argc, char **argv) {
    const char *url = argc > 1 ? argv[1] : URL;
    /* Line by line: a hung run is killed, and its output still tells. */
    setvbuf(stdout, NULL, _IOLBF, 0);
    for (int i = 0; i < 60 && !server_up(); i++) {
        sleep(1);
    }
    g_setenv("DISPLAY", ":0", TRUE);
    if (!gtk_init_check(&argc, &argv)) {
        printf("[ FAIL ] webkitgtk gtk_init\n");
        return 1;
    }
    /* Nothing kept on disk: no cookies, caches or local storage. */
    WebKitWebContext *context = webkit_web_context_new_ephemeral();
    GtkWidget *win = gtk_window_new(GTK_WINDOW_TOPLEVEL);
    gtk_window_set_default_size(GTK_WINDOW(win), 400, 300);
    view = webkit_web_view_new_with_context(context);
    g_signal_connect(view, "load-changed", G_CALLBACK(load_changed), NULL);
    g_signal_connect(view, "notify::title", G_CALLBACK(title_changed), NULL);
    g_signal_connect(view, "load-failed", G_CALLBACK(load_failed), NULL);
    g_signal_connect(view, "web-process-terminated", G_CALLBACK(web_process_terminated), NULL);
    gtk_container_add(GTK_CONTAINER(win), view);
    gtk_widget_show_all(win);
    webkit_web_view_load_uri(WEBKIT_WEB_VIEW(view), url);
    g_timeout_add_seconds(600, timeout, NULL);
    gtk_main();
    return rc;
}
