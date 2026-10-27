/* gtk3-smoke: the gtk3 package's boot test, against mlibc.
 *
 * On Xfbdev (display :0), a GTK window holds a label, a button and an
 * entry. Once the window has drawn, the button is clicked (its handler
 * runs), text typed into the entry is read back, and the label's pixels
 * are read from the window (gdk_pixbuf_get_from_window): the text is
 * there. Prints [ OK ] gtk3.
 */
#include <stdio.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <unistd.h>

#include <gtk/gtk.h>

static GtkWidget *label, *button, *entry;
static int drawn, clicked, rc = 1;

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

static void on_clicked(GtkButton *b, gpointer data) {
    (void)b;
    (void)data;
    clicked++;
}

static gboolean on_draw(GtkWidget *w, cairo_t *cr, gpointer data) {
    (void)w;
    (void)cr;
    (void)data;
    drawn = 1;
    return FALSE;
}

/* The label's dark pixels, read back from the window. */
static int label_dark(void) {
    GtkAllocation a;
    gtk_widget_get_allocation(label, &a);
    GdkPixbuf *pb = gdk_pixbuf_get_from_window(gtk_widget_get_window(label), a.x, a.y, a.width, a.height);
    if (!pb) {
        return -1;
    }
    int n = gdk_pixbuf_get_n_channels(pb), stride = gdk_pixbuf_get_rowstride(pb), dark = 0;
    const guchar *px = gdk_pixbuf_read_pixels(pb);
    for (int y = 0; y < a.height; y++) {
        for (int x = 0; x < a.width; x++) {
            const guchar *p = px + y * stride + x * n;
            dark += p[0] < 0x80 && p[1] < 0x80 && p[2] < 0x80;
        }
    }
    g_object_unref(pb);
    return dark;
}

static gboolean check(gpointer data) {
    (void)data;
    if (!drawn) {
        return G_SOURCE_CONTINUE;
    }
    /* Let the frame reach the server. */
    gdk_display_sync(gdk_display_get_default());
    gtk_button_clicked(GTK_BUTTON(button));
    gtk_entry_set_text(GTK_ENTRY(entry), "typed");
    gtk_editable_insert_text(GTK_EDITABLE(entry), " text", -1, &(gint){5});
    const char *text = gtk_entry_get_text(GTK_ENTRY(entry));
    int dark = label_dark();
    if (clicked != 1) {
        printf("[ FAIL ] gtk3 button clicked %d times\n", clicked);
    } else if (strcmp(text, "typed text") != 0) {
        printf("[ FAIL ] gtk3 entry holds \"%s\"\n", text);
    } else if (dark < 50) {
        printf("[ FAIL ] gtk3 label: %d dark pixels\n", dark);
    } else {
        printf("[ OK ] gtk3 label %d dark pixels\n", dark);
        rc = 0;
    }
    gtk_main_quit();
    return G_SOURCE_REMOVE;
}

static gboolean timeout(gpointer data) {
    (void)data;
    printf("[ FAIL ] gtk3 window never drew\n");
    gtk_main_quit();
    return G_SOURCE_REMOVE;
}

int main(int argc, char **argv) {
    for (int i = 0; i < 60 && !server_up(); i++) {
        sleep(1);
    }
    g_setenv("DISPLAY", ":0", TRUE);
    if (!gtk_init_check(&argc, &argv)) {
        printf("[ FAIL ] gtk3 gtk_init\n");
        return 1;
    }
    GtkWidget *win = gtk_window_new(GTK_WINDOW_TOPLEVEL);
    gtk_window_set_default_size(GTK_WINDOW(win), 320, 160);
    GtkWidget *box = gtk_box_new(GTK_ORIENTATION_VERTICAL, 8);
    label = gtk_label_new(NULL);
    gtk_label_set_markup(GTK_LABEL(label), "<span size=\"xx-large\">myos gtk</span>");
    button = gtk_button_new_with_label("Click");
    entry = gtk_entry_new();
    g_signal_connect(button, "clicked", G_CALLBACK(on_clicked), NULL);
    g_signal_connect_after(win, "draw", G_CALLBACK(on_draw), NULL);
    gtk_box_pack_start(GTK_BOX(box), label, TRUE, TRUE, 0);
    gtk_box_pack_start(GTK_BOX(box), button, FALSE, FALSE, 0);
    gtk_box_pack_start(GTK_BOX(box), entry, FALSE, FALSE, 0);
    gtk_container_add(GTK_CONTAINER(win), box);
    gtk_widget_show_all(win);
    g_timeout_add(100, check, NULL);
    g_timeout_add_seconds(60, timeout, NULL);
    gtk_main();
    return rc;
}
