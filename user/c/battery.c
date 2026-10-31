/* battery: the batteries' charge and the AC adapters' state, one line
 * each, from /proc/acpi/battery (the acpi module, x86_64; docs/platform.md):
 *
 *   BAT0: 83% discharging, 2:41 left
 *   AC: online
 *
 * -r prints the file as it is. Exits 1 when the machine reports no battery
 * and no adapter (the file is missing).
 */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define PROC "/proc/acpi/battery"
#define MAX 8

/* A device's keys from the file (`NAME KEY VALUE [UNIT]` lines). */
struct dev {
    char name[16];
    int battery;   /* it has battery keys (else an adapter) */
    int present;
    char state[24];
    long percent, remaining, full, rate;
    char online[16];
};

static struct dev devs[MAX];
static int ndevs;

static struct dev *dev(const char *name) {
    for (int i = 0; i < ndevs; i++)
        if (strcmp(devs[i].name, name) == 0)
            return &devs[i];
    if (ndevs == MAX)
        return NULL;
    struct dev *d = &devs[ndevs++];
    memset(d, 0, sizeof *d);
    snprintf(d->name, sizeof d->name, "%s", name);
    d->percent = d->remaining = d->full = d->rate = -1;
    return d;
}

int main(int argc, char **argv) {
    int raw = argc > 1 && strcmp(argv[1], "-r") == 0;
    if (argc > 1 && !raw) {
        fprintf(stderr, "usage: battery [-r]\n");
        return 2;
    }
    FILE *f = fopen(PROC, "r");
    if (!f) {
        fprintf(stderr, "battery: no battery or AC adapter (" PROC ")\n");
        return 1;
    }
    char line[128];
    while (fgets(line, sizeof line, f)) {
        if (raw) {
            fputs(line, stdout);
            continue;
        }
        char name[16], key[16], value[24];
        if (sscanf(line, "%15s %15s %23s", name, key, value) != 3)
            continue;
        if (strcmp(name, "error") == 0) {
            fprintf(stderr, "battery: %s", line + 6);
            fclose(f);
            return 1;
        }
        struct dev *d = dev(name);
        if (!d)
            continue;
        long n = strtol(value, NULL, 10);
        if (strcmp(key, "online") == 0) {
            snprintf(d->online, sizeof d->online, "%s", value);
            continue;
        }
        d->battery = 1;
        if (strcmp(key, "present") == 0)
            d->present = n != 0;
        else if (strcmp(key, "state") == 0)
            snprintf(d->state, sizeof d->state, "%s", value);
        else if (strcmp(key, "percent") == 0)
            d->percent = n;
        else if (strcmp(key, "remaining") == 0)
            d->remaining = n;
        else if (strcmp(key, "full") == 0)
            d->full = n;
        else if (strcmp(key, "rate") == 0)
            d->rate = n;
    }
    fclose(f);
    for (int i = 0; i < ndevs && !raw; i++) {
        struct dev *d = &devs[i];
        if (!d->battery) {
            printf("%s: %s\n", d->name,
                   strcmp(d->online, "1") == 0 ? "online"
                   : strcmp(d->online, "0") == 0 ? "offline" : "unknown");
            continue;
        }
        if (!d->present) {
            printf("%s: not present\n", d->name);
            continue;
        }
        printf("%s:", d->name);
        if (d->percent >= 0)
            printf(" %ld%%", d->percent);
        printf(" %s", d->state[0] ? d->state : "unknown");
        /* The time left at the present rate: to empty when discharging,
         * to full when charging (capacity and rate in one unit, mWh and
         * mW or mAh and mA). */
        long left = -1;
        if (d->rate > 0 && d->remaining >= 0) {
            if (strcmp(d->state, "discharging") == 0)
                left = d->remaining * 60 / d->rate;
            else if (strcmp(d->state, "charging") == 0 && d->full > d->remaining)
                left = (d->full - d->remaining) * 60 / d->rate;
        }
        if (left >= 0)
            printf(", %ld:%02ld %s", left / 60, left % 60,
                   strcmp(d->state, "charging") == 0 ? "until full" : "left");
        printf("\n");
    }
    return 0;
}
