/* GLib against mlibc on myos (packages/glib, its test.sh): the main loop
 * (a timeout, an idle and a source another thread feeds), threads and an
 * async queue, a GObject with a property and a signal, GRegex (PCRE2),
 * GIO's files (write, read, enumerate), a GSocket echo over loopback,
 * g_spawn_sync and g_convert (GNU libiconv). It prints one line per part
 * and exits nonzero at the first that fails. */
#include <gio/gio.h>
#include <string.h>
#include <sys/wait.h>

#define CHECK(cond, ...) \
	do { \
		if (!(cond)) { \
			g_printerr("glib-smoke: " __VA_ARGS__); \
			g_printerr("\n"); \
			return 1; \
		} \
	} while (0)

/* The main loop. */

struct loop_state {
	GMainLoop *loop;
	int timeouts, idles, invoked;
};

static gboolean on_timeout(gpointer data)
{
	struct loop_state *s = data;
	return ++s->timeouts < 3 ? G_SOURCE_CONTINUE : G_SOURCE_REMOVE;
}

static gboolean on_idle(gpointer data)
{
	struct loop_state *s = data;
	s->idles++;
	return G_SOURCE_REMOVE;
}

static gboolean on_invoke(gpointer data)
{
	struct loop_state *s = data;
	s->invoked = 1;
	g_main_loop_quit(s->loop);
	return G_SOURCE_REMOVE;
}

/* Waits for the loop's three timeouts, then wakes it from this thread. */
static gpointer feeder(gpointer data)
{
	struct loop_state *s = data;
	while (g_atomic_int_get(&s->timeouts) < 3)
		g_usleep(10 * 1000);
	g_main_context_invoke(NULL, on_invoke, s);
	return NULL;
}

static int test_main_loop(void)
{
	struct loop_state s = { g_main_loop_new(NULL, FALSE), 0, 0, 0 };
	g_timeout_add(20, on_timeout, &s);
	g_idle_add(on_idle, &s);
	GThread *t = g_thread_new("feeder", feeder, &s);
	g_main_loop_run(s.loop);
	g_thread_join(t);
	g_main_loop_unref(s.loop);
	CHECK(s.timeouts == 3 && s.idles == 1 && s.invoked, "main loop: %d timeouts, %d idles, invoked %d",
	      s.timeouts, s.idles, s.invoked);
	g_print("main loop ok\n");
	return 0;
}

/* Threads: four producers into one async queue. */

static gpointer producer(gpointer data)
{
	GAsyncQueue *q = data;
	for (int i = 1; i <= 100; i++)
		g_async_queue_push(q, GINT_TO_POINTER(i));
	return NULL;
}

static int test_threads(void)
{
	GAsyncQueue *q = g_async_queue_new();
	GThread *t[4];
	int sum = 0;
	for (int i = 0; i < 4; i++)
		t[i] = g_thread_new("producer", producer, q);
	for (int i = 0; i < 400; i++)
		sum += GPOINTER_TO_INT(g_async_queue_pop(q));
	for (int i = 0; i < 4; i++)
		g_thread_join(t[i]);
	g_async_queue_unref(q);
	CHECK(sum == 4 * 5050, "threads: sum %d", sum);
	g_print("threads ok\n");
	return 0;
}

/* GObject: a counter with a "value" property and a "changed" signal. */

#define SMOKE_TYPE_COUNTER (smoke_counter_get_type())
G_DECLARE_FINAL_TYPE(SmokeCounter, smoke_counter, SMOKE, COUNTER, GObject)

struct _SmokeCounter {
	GObject parent;
	int value;
};

G_DEFINE_TYPE(SmokeCounter, smoke_counter, G_TYPE_OBJECT)

enum { PROP_VALUE = 1 };
static guint changed_signal;

static void smoke_counter_set_property(GObject *obj, guint id, const GValue *v, GParamSpec *pspec)
{
	SmokeCounter *c = SMOKE_COUNTER(obj);
	if (id != PROP_VALUE) {
		G_OBJECT_WARN_INVALID_PROPERTY_ID(obj, id, pspec);
		return;
	}
	c->value = g_value_get_int(v);
	g_signal_emit(c, changed_signal, 0, c->value);
}

static void smoke_counter_get_property(GObject *obj, guint id, GValue *v, GParamSpec *pspec)
{
	if (id != PROP_VALUE) {
		G_OBJECT_WARN_INVALID_PROPERTY_ID(obj, id, pspec);
		return;
	}
	g_value_set_int(v, SMOKE_COUNTER(obj)->value);
}

static void smoke_counter_class_init(SmokeCounterClass *klass)
{
	GObjectClass *oc = G_OBJECT_CLASS(klass);
	oc->set_property = smoke_counter_set_property;
	oc->get_property = smoke_counter_get_property;
	g_object_class_install_property(oc, PROP_VALUE,
		g_param_spec_int("value", NULL, NULL, 0, 1000, 0, G_PARAM_READWRITE));
	/* No marshaller: the generic one, through libffi. */
	changed_signal = g_signal_new("changed", G_TYPE_FROM_CLASS(klass), G_SIGNAL_RUN_LAST, 0,
				      NULL, NULL, NULL, G_TYPE_NONE, 1, G_TYPE_INT);
}

static void smoke_counter_init(SmokeCounter *c)
{
	(void)c;
}

static void on_changed(SmokeCounter *c, int value, gpointer data)
{
	(void)c;
	*(int *)data += value;
}

static int test_gobject(void)
{
	int seen = 0, got = 0;
	SmokeCounter *c = g_object_new(SMOKE_TYPE_COUNTER, "value", 1, NULL);
	g_signal_connect(c, "changed", G_CALLBACK(on_changed), &seen);
	g_object_set(c, "value", 42, NULL);
	g_object_get(c, "value", &got, NULL);
	g_object_unref(c);
	CHECK(seen == 42 && got == 42, "gobject: signal saw %d, property %d", seen, got);
	g_print("gobject ok\n");
	return 0;
}

/* GRegex. */

static int test_regex(void)
{
	GError *err = NULL;
	GRegex *re = g_regex_new("(\\w+)@(\\w+)\\.org", G_REGEX_DEFAULT, G_REGEX_MATCH_DEFAULT, &err);
	CHECK(re, "regex: %s", err->message);
	GMatchInfo *m;
	g_regex_match(re, "mail gnome@gnome.org now", 0, &m);
	char *user = g_match_info_fetch(m, 1);
	char *swapped = g_regex_replace(re, "a@b.org", -1, 0, "\\2@\\1", 0, NULL);
	int ok = g_match_info_matches(m) && !strcmp(user, "gnome") && !strcmp(swapped, "b@a");
	g_free(user);
	g_free(swapped);
	g_match_info_free(m);
	g_regex_unref(re);
	CHECK(ok, "regex: no match");
	g_print("regex ok\n");
	return 0;
}

/* GIO: a file written and read back, a directory enumerated. */

static int test_gio_files(const char *dir)
{
	GError *err = NULL;
	char *path = g_build_filename(dir, "glib-smoke.txt", NULL);
	GFile *f = g_file_new_for_path(path);
	CHECK(g_file_replace_contents(f, "hello gio\n", 10, NULL, FALSE, G_FILE_CREATE_NONE, NULL, NULL,
				      &err), "gio write: %s", err->message);
	char *data;
	gsize len;
	CHECK(g_file_load_contents(f, NULL, &data, &len, NULL, &err), "gio read: %s", err->message);
	CHECK(len == 10 && !memcmp(data, "hello gio\n", 10), "gio read: %zu bytes", (size_t)len);
	g_free(data);

	GFile *d = g_file_new_for_path(dir);
	GFileEnumerator *e = g_file_enumerate_children(d, G_FILE_ATTRIBUTE_STANDARD_NAME "," G_FILE_ATTRIBUTE_STANDARD_SIZE,
						       G_FILE_QUERY_INFO_NONE, NULL, &err);
	CHECK(e, "gio enumerate: %s", err->message);
	int found = 0;
	GFileInfo *info;
	while ((info = g_file_enumerator_next_file(e, NULL, NULL))) {
		if (!strcmp(g_file_info_get_name(info), "glib-smoke.txt") && g_file_info_get_size(info) == 10)
			found = 1;
		g_object_unref(info);
	}
	g_object_unref(e);
	CHECK(g_file_delete(f, NULL, &err), "gio delete: %s", err->message);
	g_object_unref(d);
	g_object_unref(f);
	g_free(path);
	CHECK(found, "gio enumerate: glib-smoke.txt not listed");
	g_print("gio files ok\n");
	return 0;
}

/* GSocket: a loopback echo, the server a thread. */

static gpointer echo_server(gpointer data)
{
	GSocket *listener = data;
	GSocket *s = g_socket_accept(listener, NULL, NULL);
	char buf[64];
	gssize n;
	if (!s)
		return NULL;
	while ((n = g_socket_receive(s, buf, sizeof buf, NULL, NULL)) > 0)
		g_socket_send(s, buf, n, NULL, NULL);
	g_object_unref(s);
	return NULL;
}

static int test_socket(void)
{
	GError *err = NULL;
	GSocket *listener = g_socket_new(G_SOCKET_FAMILY_IPV4, G_SOCKET_TYPE_STREAM, G_SOCKET_PROTOCOL_TCP, &err);
	CHECK(listener, "socket: %s", err->message);
	GInetAddress *lo = g_inet_address_new_loopback(G_SOCKET_FAMILY_IPV4);
	GSocketAddress *any = g_inet_socket_address_new(lo, 0);
	CHECK(g_socket_bind(listener, any, TRUE, &err) && g_socket_listen(listener, &err), "socket listen: %s",
	      err->message);
	GSocketAddress *addr = g_socket_get_local_address(listener, &err);
	CHECK(addr, "socket address: %s", err->message);
	GThread *t = g_thread_new("echo", echo_server, listener);

	GSocketClient *client = g_socket_client_new();
	GSocketConnection *conn = g_socket_client_connect(client, G_SOCKET_CONNECTABLE(addr), NULL, &err);
	CHECK(conn, "socket connect: %s", err->message);
	GOutputStream *out = g_io_stream_get_output_stream(G_IO_STREAM(conn));
	GInputStream *in = g_io_stream_get_input_stream(G_IO_STREAM(conn));
	char buf[16] = { 0 };
	gsize n;
	CHECK(g_output_stream_write_all(out, "ping myos", 9, NULL, NULL, &err), "socket send: %s", err->message);
	CHECK(g_input_stream_read_all(in, buf, 9, &n, NULL, &err) && n == 9, "socket receive: %s",
	      err ? err->message : "short");
	g_io_stream_close(G_IO_STREAM(conn), NULL, NULL);
	g_thread_join(t);
	g_object_unref(conn);
	g_object_unref(client);
	g_object_unref(addr);
	g_object_unref(any);
	g_object_unref(lo);
	g_object_unref(listener);
	CHECK(!strcmp(buf, "ping myos"), "socket echo: %s", buf);
	g_print("socket ok\n");
	return 0;
}

/* g_spawn_sync and g_convert. */

static int test_spawn_convert(void)
{
	GError *err = NULL;
	char *argv[] = { "sh", "-c", "echo spawned; exit 3", NULL };
	char *out = NULL;
	int status;
	CHECK(g_spawn_sync(NULL, argv, NULL, G_SPAWN_SEARCH_PATH, NULL, NULL, &out, NULL, &status, &err),
	      "spawn: %s", err->message);
	CHECK(!strcmp(out, "spawned\n") && WIFEXITED(status) && WEXITSTATUS(status) == 3, "spawn: '%s', status %d",
	      out, status);
	g_free(out);

	gsize n;
	char *latin1 = g_convert("caf\xc3\xa9", -1, "ISO-8859-1", "UTF-8", NULL, &n, &err);
	CHECK(latin1, "convert: %s", err->message);
	CHECK(n == 4 && (unsigned char)latin1[3] == 0xe9, "convert: %zu bytes", (size_t)n);
	char *back = g_convert(latin1, n, "UTF-8", "ISO-8859-1", NULL, NULL, &err);
	CHECK(back && !strcmp(back, "caf\xc3\xa9"), "convert back: %s", err ? err->message : back);
	g_free(latin1);
	g_free(back);
	g_print("spawn convert ok\n");
	return 0;
}

int main(int argc, char **argv)
{
	const char *dir = argc > 1 ? argv[1] : "/tmp";
	return test_main_loop() || test_threads() || test_gobject() || test_regex() || test_gio_files(dir) ||
	       test_socket() || test_spawn_convert();
}
