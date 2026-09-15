/* myos libgloss: POSIX search.h functions newlib does not ship.
 * lsearch/lfind (linear search) and insque/remque (doubly-linked list).
 * Prototypes come from <search.h>. */

#include <search.h>
#include <string.h>

/* lfind: linear search; returns match or NULL. Never modifies *nelp. */
void *
lfind(const void *key, const void *base, size_t *nelp, size_t width,
      int (*compar)(const void *, const void *))
{
    const char *p = base;
    size_t n;

    if (base == NULL || nelp == NULL || compar == NULL)
        return NULL;
    for (n = *nelp; n > 0; n--, p += width) {
        if (compar(key, p) == 0)
            return (void *)p;
    }
    return NULL;
}

/* lsearch: lfind, appending key when absent (*nelp grows). */
void *
lsearch(const void *key, void *base, size_t *nelp, size_t width,
        int (*compar)(const void *, const void *))
{
    void *hit;

    if (nelp == NULL)
        return NULL;
    hit = lfind(key, base, nelp, width, compar);
    if (hit != NULL)
        return hit;
    if (base == NULL || compar == NULL)
        return NULL;
    memcpy((char *)base + (*nelp * width), key, width);
    (*nelp)++;
    return (char *)base + ((*nelp - 1) * width);
}

/* insque/remque operate on struct qelem (search.h): {forw, back, data}. */
void
insque(void *element, void *pred)
{
    struct qelem *e = element;
    struct qelem *p = pred;

    if (e == NULL)
        return;
    if (p == NULL) {
        e->q_forw = NULL;
        e->q_back = NULL;
        return;
    }
    e->q_forw = p->q_forw;
    e->q_back = p;
    if (p->q_forw != NULL)
        p->q_forw->q_back = e;
    p->q_forw = e;
}

void
remque(void *element)
{
    struct qelem *e = element;

    if (e == NULL)
        return;
    if (e->q_back != NULL)
        e->q_back->q_forw = e->q_forw;
    if (e->q_forw != NULL)
        e->q_forw->q_back = e->q_back;
}
