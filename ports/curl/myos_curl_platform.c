/* Entropy + time for mbedtls when linked into curl (mirrors user/tls platform). */
#include <stddef.h>
#include <stdint.h>
#include <sys/time.h>
#include <time.h>

#include "mbedtls/platform.h"
#include "mbedtls/platform_time.h"

int mbedtls_hardware_poll(void *data, unsigned char *output, size_t len, size_t *olen) {
    (void)data;
    struct timeval tv;
    uint64_t s = 0;
    if (gettimeofday(&tv, NULL) == 0) {
        s = ((uint64_t)tv.tv_sec << 32) ^ (uint64_t)tv.tv_usec;
    }
    s ^= (uint64_t)(uintptr_t)output << 7;
    s ^= (uint64_t)(uintptr_t)&s << 13;
    s ^= 0xA5A5F00DDEADBEEFULL;
    for (size_t i = 0; i < len; i++) {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        s += 0x9E3779B97F4A7C15ULL;
        output[i] = (unsigned char)(s >> 32);
    }
    if (olen) {
        *olen = len;
    }
    return 0;
}

mbedtls_ms_time_t mbedtls_ms_time(void) {
    struct timeval tv;
    if (gettimeofday(&tv, NULL) != 0) {
        return 0;
    }
    return (mbedtls_ms_time_t)tv.tv_sec * 1000 + (mbedtls_ms_time_t)(tv.tv_usec / 1000);
}
