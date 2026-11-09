/* libsoup-smoke: the libsoup package's boot test, against mlibc.
 *
 * glib-networking's OpenSSL backend, linked in, is registered as GIO's TLS
 * backend (a static program loads no GIO module). Then libsoup GETs
 * /hello.txt from the launcher's test server (src/test_www.rs) over HTTP,
 * and over HTTPS with the test CA as its TLS database; over HTTPS with the
 * image's CA bundle instead, the server's certificate is refused. libxml2
 * parses a document and libpsl finds a registrable domain in its built-in
 * list. Prints [ OK ] libsoup.
 */
#include <stdio.h>
#include <string.h>

#include <libpsl.h>
#include <libsoup/soup.h>
#include <libxml/parser.h>
#include <libxml/tree.h>

#define HELLO "hello from the myos test server\n"
#define TEST_CA "/lib/myos-tests/test-ca.pem"

/* glib-networking's OpenSSL module entry (gioopenssl, static): with no
 * module, it registers the backend for the program. */
void g_io_openssl_load(GIOModule *module);

static int fail(const char *what) {
    printf("[ FAIL ] libsoup %s\n", what);
    return 1;
}

/* GET URL with SESSION: 0 when the body is HELLO; else 1, ERR set when
 * the request itself failed. */
static int get(SoupSession *session, const char *url, GError **err) {
    SoupMessage *msg = soup_message_new("GET", url);
    GBytes *body = soup_session_send_and_read(session, msg, NULL, err);
    int rc = 1;
    if (body) {
        gsize n;
        const char *data = g_bytes_get_data(body, &n);
        guint status = soup_message_get_status(msg);
        if (status == SOUP_STATUS_OK && n == strlen(HELLO) && memcmp(data, HELLO, n) == 0) {
            rc = 0;
        } else {
            printf("[ FAIL ] libsoup %s: status %u, %zu bytes\n", url, status, (size_t)n);
        }
        g_bytes_unref(body);
    }
    g_object_unref(msg);
    return rc;
}

static int xml(void) {
    const char doc[] = "<root><item name=\"myos\">libsoup</item></root>";
    xmlDocPtr d = xmlReadMemory(doc, sizeof doc - 1, "smoke.xml", NULL, XML_PARSE_NONET);
    if (!d) {
        return fail("xmlReadMemory");
    }
    xmlNodePtr root = xmlDocGetRootElement(d);
    xmlNodePtr item = root ? xmlFirstElementChild(root) : NULL;
    xmlChar *name = item ? xmlGetProp(item, (const xmlChar *)"name") : NULL;
    xmlChar *text = item ? xmlNodeGetContent(item) : NULL;
    int ok = root && strcmp((const char *)root->name, "root") == 0 && name &&
             strcmp((const char *)name, "myos") == 0 && text && strcmp((const char *)text, "libsoup") == 0;
    xmlFree(name);
    xmlFree(text);
    xmlFreeDoc(d);
    return ok ? 0 : fail("libxml2 tree");
}

static int psl(void) {
    const psl_ctx_t *psl = psl_builtin();
    const char *reg = psl ? psl_registrable_domain(psl, "www.example.co.uk") : NULL;
    if (!reg || strcmp(reg, "example.co.uk") != 0 || !psl_is_public_suffix(psl, "co.uk")) {
        return fail("libpsl registrable domain");
    }
    return 0;
}

int main(void) {
    GError *err = NULL;
    if (xml() || psl()) {
        return 1;
    }
    g_io_openssl_load(NULL);
    if (!g_tls_backend_supports_tls(g_tls_backend_get_default())) {
        return fail("no TLS backend");
    }

    SoupSession *plain = soup_session_new_with_options("timeout", 60, NULL);
    if (get(plain, "http://10.0.2.2:8766/hello.txt", &err)) {
        printf("[ FAIL ] libsoup http: %s\n", err ? err->message : "body");
        return 1;
    }

    GTlsDatabase *ca = g_tls_file_database_new(TEST_CA, &err);
    if (!ca) {
        printf("[ FAIL ] libsoup %s: %s\n", TEST_CA, err->message);
        return 1;
    }
    SoupSession *tls = soup_session_new_with_options("timeout", 60, "tls-database", ca, NULL);
    if (get(tls, "https://10.0.2.2:8767/hello.txt", &err)) {
        printf("[ FAIL ] libsoup https: %s\n", err ? err->message : "body");
        return 1;
    }

    /* The image's bundle does not hold the test CA: the handshake fails. */
    SoupSession *bundle = soup_session_new_with_options("timeout", 60, NULL);
    if (!get(bundle, "https://10.0.2.2:8767/hello.txt", &err) || !err || err->domain != G_TLS_ERROR) {
        printf("[ FAIL ] libsoup https without the test CA: %s\n", err ? err->message : "accepted");
        return 1;
    }
    g_clear_error(&err);

    g_object_unref(bundle);
    g_object_unref(tls);
    g_object_unref(ca);
    g_object_unref(plain);
    printf("[ OK ] libsoup http, https, libxml2, libpsl\n");
    return 0;
}
