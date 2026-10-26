/* play, rec: the sound server's programs (docs/sound.md). They talk to
 * `sndd` over its unix socket: a request line (`play RATE CHANNELS FORMAT
 * [STREAM]` or `record ...`), its answer (`ok`), then the PCM.
 *
 *   play [-D STREAM] [-r RATE] [-c CHANNELS] [-f u8|s16] -t HZ[,HZ...] [-d SECONDS]
 *       a tone (several, summed), 1 s by default
 *   play [-D STREAM] FILE.wav
 *       a WAV file (PCM, 8 or 16 bits)
 *   rec [-D STREAM] [-r RATE] [-c CHANNELS] [-f u8|s16] -d SECONDS FILE.wav
 *       what the capture stream hears, into a WAV file; `rec -n` says how
 *       many frames came instead
 *   play -l
 *       the streams (`N playback|capture CARD`)
 *
 * The defaults are 48000 Hz, 2 channels, s16. */
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <time.h>
#include <unistd.h>

static const char *prog = "play";

static int die(const char *what) {
    fprintf(stderr, "%s: %s\n", prog, what);
    return 1;
}

static uint64_t now_ns(void) {
    struct timespec t;
    clock_gettime(CLOCK_MONOTONIC, &t);
    return (uint64_t)t.tv_sec * 1000000000ull + (uint64_t)t.tv_nsec;
}

/* A connection to the server, `request` sent and its answer read: the fd,
 * or -1 (the answer printed). */
static int connect_server(const char *request) {
    int fd = socket(AF_UNIX, SOCK_STREAM, 0);
    if (fd < 0)
        return -1;
    struct sockaddr_un sun;
    memset(&sun, 0, sizeof sun);
    sun.sun_family = AF_UNIX;
    strcpy(sun.sun_path, "sndd");
    if (connect(fd, (struct sockaddr *)&sun, sizeof sun) < 0) {
        fprintf(stderr, "%s: no sound server (sndd): errno %d\n", prog, errno);
        close(fd);
        return -1;
    }
    size_t len = strlen(request);
    if (write(fd, request, len) != (ssize_t)len) {
        close(fd);
        return -1;
    }
    return fd;
}

/* The server's answer line into `buf`: 1 for `ok`. */
static int answer(int fd, char *buf, size_t cap) {
    size_t n = 0;
    while (n + 1 < cap) {
        struct pollfd p = {.fd = fd, .events = POLLIN};
        if (poll(&p, 1, 5000) != 1)
            break;
        char c;
        if (read(fd, &c, 1) != 1)
            break;
        if (c == '\n')
            break;
        buf[n++] = c;
    }
    buf[n] = 0;
    return strcmp(buf, "ok") == 0;
}

/* All of `buf` to the socket, waiting while it is full. */
static int write_all(int fd, const uint8_t *buf, size_t len) {
    while (len) {
        struct pollfd p = {.fd = fd, .events = POLLOUT};
        if (poll(&p, 1, 5000) != 1)
            return -1;
        ssize_t k = write(fd, buf, len);
        if (k < 0 && errno != EAGAIN)
            return -1;
        if (k > 0) {
            buf += k;
            len -= (size_t)k;
        }
    }
    return 0;
}

/* sin(x) for |x| <= pi, by its series (no libm here). */
static double sine(double x) {
    double x2 = x * x, term = x, sum = x;
    for (int k = 1; k < 12; k++) {
        term *= -x2 / ((2 * k) * (2 * k + 1));
        sum += term;
    }
    return sum;
}

static const double PI = 3.14159265358979323846;

static void put_sample(uint8_t **p, int fmt16, int v) {
    if (v > 32767)
        v = 32767;
    if (v < -32768)
        v = -32768;
    if (fmt16) {
        (*p)[0] = (uint8_t)v;
        (*p)[1] = (uint8_t)(v >> 8);
        *p += 2;
    } else {
        *(*p)++ = (uint8_t)((v >> 8) + 128);
    }
}

static int play_tone(const char *stream, int rate, int channels, int fmt16, const char *freqs, double seconds) {
    double f[8];
    int nf = 0;
    for (const char *s = freqs; *s && nf < 8;) {
        f[nf++] = strtod(s, (char **)&s);
        if (*s == ',')
            s++;
        else
            break;
    }
    if (nf == 0 || rate <= 0)
        return die("-t HZ[,HZ...]");
    char req[96];
    snprintf(req, sizeof req, "play %d %d %s%s%s\n", rate, channels, fmt16 ? "s16" : "u8", stream ? " " : "", stream ? stream : "");
    int fd = connect_server(req);
    if (fd < 0)
        return 1;
    char ans[128];
    if (!answer(fd, ans, sizeof ans)) {
        fprintf(stderr, "%s: %s\n", prog, ans);
        return 1;
    }
    long frames = (long)(seconds * rate);
    double phase[8] = {0};
    uint8_t buf[4096];
    int frame = channels * (fmt16 ? 2 : 1);
    long done = 0;
    while (done < frames) {
        uint8_t *p = buf;
        while (done < frames && p + frame <= buf + sizeof buf) {
            double v = 0;
            for (int i = 0; i < nf; i++) {
                v += sine(phase[i] - PI);
                phase[i] += 2 * PI * f[i] / rate;
                if (phase[i] >= 2 * PI)
                    phase[i] -= 2 * PI;
            }
            /* sin(x - pi) = -sin(x): the sign does not matter here. */
            int s = (int)(v / nf * 12000);
            for (int c = 0; c < channels; c++)
                put_sample(&p, fmt16, s);
            done++;
        }
        if (write_all(fd, buf, (size_t)(p - buf)) < 0) {
            close(fd);
            return die("the server went away");
        }
    }
    close(fd);
    return 0;
}

static uint32_t le32(const uint8_t *p) {
    return p[0] | p[1] << 8 | p[2] << 16 | (uint32_t)p[3] << 24;
}

static int play_file(const char *stream, const char *path) {
    int in = open(path, O_RDONLY);
    if (in < 0)
        return die("cannot open the file");
    uint8_t hdr[12];
    if (read(in, hdr, 12) != 12 || memcmp(hdr, "RIFF", 4) || memcmp(hdr + 8, "WAVE", 4))
        return die("not a WAV file");
    int rate = 0, channels = 0, bits = 0;
    uint8_t chunk[8];
    for (;;) {
        if (read(in, chunk, 8) != 8)
            return die("no data chunk");
        uint32_t len = le32(chunk + 4);
        if (!memcmp(chunk, "fmt ", 4)) {
            uint8_t fmt[16];
            if (len < 16 || read(in, fmt, 16) != 16)
                return die("bad fmt chunk");
            if ((fmt[0] | fmt[1] << 8) != 1)
                return die("not PCM");
            channels = fmt[2] | fmt[3] << 8;
            rate = (int)le32(fmt + 4);
            bits = fmt[14] | fmt[15] << 8;
            lseek(in, len - 16 + (len & 1), SEEK_CUR);
        } else if (!memcmp(chunk, "data", 4)) {
            break;
        } else {
            lseek(in, len + (len & 1), SEEK_CUR);
        }
    }
    if (!rate || (bits != 8 && bits != 16) || channels < 1)
        return die("unsupported WAV format");
    char req[96];
    snprintf(req, sizeof req, "play %d %d %s%s%s\n", rate, channels, bits == 16 ? "s16" : "u8", stream ? " " : "", stream ? stream : "");
    int fd = connect_server(req);
    if (fd < 0)
        return 1;
    char ans[128];
    if (!answer(fd, ans, sizeof ans)) {
        fprintf(stderr, "%s: %s\n", prog, ans);
        return 1;
    }
    uint8_t buf[8192];
    ssize_t n;
    while ((n = read(in, buf, sizeof buf)) > 0)
        if (write_all(fd, buf, (size_t)n) < 0)
            return die("the server went away");
    close(fd);
    return 0;
}

static void put32(uint8_t *p, uint32_t v) {
    p[0] = (uint8_t)v;
    p[1] = (uint8_t)(v >> 8);
    p[2] = (uint8_t)(v >> 16);
    p[3] = (uint8_t)(v >> 24);
}

static int record(const char *stream, int rate, int channels, int fmt16, double seconds, const char *path, int count) {
    char req[96];
    snprintf(req, sizeof req, "record %d %d %s%s%s\n", rate, channels, fmt16 ? "s16" : "u8", stream ? " " : "", stream ? stream : "");
    int fd = connect_server(req);
    if (fd < 0)
        return 1;
    char ans[128];
    if (!answer(fd, ans, sizeof ans)) {
        fprintf(stderr, "%s: %s\n", prog, ans);
        return 1;
    }
    int out = -1;
    uint8_t hdr[44];
    if (!count) {
        out = open(path, O_WRONLY | O_CREAT | O_TRUNC, 0644);
        if (out < 0)
            return die("cannot create the file");
        memset(hdr, 0, sizeof hdr);
        if (write(out, hdr, sizeof hdr) != sizeof hdr)
            return die("cannot write the file");
    }
    int frame = channels * (fmt16 ? 2 : 1);
    uint64_t total = 0, end = now_ns() + (uint64_t)(seconds * 1e9);
    uint8_t buf[8192];
    for (;;) {
        uint64_t t = now_ns();
        if (t >= end)
            break;
        struct pollfd p = {.fd = fd, .events = POLLIN};
        int wait = (int)((end - t) / 1000000) + 1;
        if (poll(&p, 1, wait) != 1)
            continue;
        ssize_t n = read(fd, buf, sizeof buf);
        if (n <= 0)
            break;
        total += (uint64_t)n;
        if (out >= 0 && write(out, buf, (size_t)n) != n)
            return die("cannot write the file");
    }
    close(fd);
    if (count) {
        printf("%llu frames\n", (unsigned long long)(total / frame));
        return 0;
    }
    memcpy(hdr, "RIFF", 4);
    put32(hdr + 4, (uint32_t)(36 + total));
    memcpy(hdr + 8, "WAVEfmt ", 8);
    put32(hdr + 16, 16);
    hdr[20] = 1;
    hdr[22] = (uint8_t)channels;
    put32(hdr + 24, (uint32_t)rate);
    put32(hdr + 28, (uint32_t)(rate * frame));
    hdr[32] = (uint8_t)frame;
    hdr[34] = fmt16 ? 16 : 8;
    memcpy(hdr + 36, "data", 4);
    put32(hdr + 40, (uint32_t)total);
    lseek(out, 0, SEEK_SET);
    if (write(out, hdr, sizeof hdr) != sizeof hdr)
        return die("cannot write the file");
    close(out);
    return 0;
}

static int list(void) {
    int fd = connect_server("devices\n");
    if (fd < 0)
        return 1;
    char buf[1024];
    ssize_t n;
    struct pollfd p = {.fd = fd, .events = POLLIN};
    while (poll(&p, 1, 5000) == 1 && (n = read(fd, buf, sizeof buf)) > 0)
        fwrite(buf, 1, (size_t)n, stdout);
    close(fd);
    return 0;
}

static int usage(void) {
    fprintf(stderr,
            "usage: play [-D STREAM] [-r RATE] [-c CHANNELS] [-f u8|s16] -t HZ[,HZ...] [-d SECONDS]\n"
            "       play [-D STREAM] FILE.wav | play -l\n"
            "       rec [-D STREAM] [-r RATE] [-c CHANNELS] [-f u8|s16] -d SECONDS (FILE.wav | -n)\n");
    return 2;
}

int main(int argc, char **argv) {
    const char *base = strrchr(argv[0], '/');
    base = base ? base + 1 : argv[0];
    int recording = strcmp(base, "rec") == 0;
    prog = recording ? "rec" : "play";
    const char *stream = NULL, *freqs = NULL;
    int rate = 48000, channels = 2, fmt16 = 1, count = 0;
    double seconds = -1;
    int i = 1;
    for (; i < argc && argv[i][0] == '-' && argv[i][1]; i++) {
        const char *a = argv[i];
        const char *v = i + 1 < argc ? argv[i + 1] : NULL;
        if (!strcmp(a, "-l"))
            return list();
        if (!strcmp(a, "-n")) {
            count = 1;
            continue;
        }
        if (!v)
            return usage();
        if (!strcmp(a, "-D"))
            stream = v;
        else if (!strcmp(a, "-r"))
            rate = atoi(v);
        else if (!strcmp(a, "-c"))
            channels = atoi(v);
        else if (!strcmp(a, "-f"))
            fmt16 = strcmp(v, "u8") != 0;
        else if (!strcmp(a, "-t"))
            freqs = v;
        else if (!strcmp(a, "-d"))
            seconds = strtod(v, NULL);
        else
            return usage();
        i++;
    }
    if (channels < 1 || channels > 8)
        return usage();
    if (recording) {
        if (seconds <= 0 || (!count && i >= argc))
            return usage();
        return record(stream, rate, channels, fmt16, seconds, count ? NULL : argv[i], count);
    }
    if (freqs)
        return play_tone(stream, rate, channels, fmt16, freqs, seconds > 0 ? seconds : 1.0);
    if (i < argc)
        return play_file(stream, argv[i]);
    return usage();
}
