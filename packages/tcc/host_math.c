/*
 * Minimal host math so TinyCC can link without libm.
 *
 * tccpp.c uses ldexpl/strtold while folding numeric constants. newlib libm
 * pulls scalbnl, which needs IEEE-128 compiler-rt on aarch64/riscv64.
 * Integer ldexp avoids both libm and recursive soft-float libcalls.
 */
#include <stdint.h>

double strtod(const char *nptr, char **endptr);

double ldexp(double x, int n);
long double ldexpl(long double x, int n);
double scalbn(double x, int n);
long double scalbnl(long double x, int n);
long double strtold(const char *nptr, char **endptr);

double ldexp(double x, int n)
{
	union {
		double d;
		uint64_t u;
	} v;
	uint64_t sign;
	uint64_t frac;
	int exp;
	int64_t e;

	v.d = x;
	sign = v.u & 0x8000000000000000ULL;
	exp = (int)((v.u >> 52) & 0x7ff);
	frac = v.u & 0x000fffffffffffffULL;

	if (exp == 0x7ff) {
		return x;
	}
	if (exp == 0) {
		if (frac == 0) {
			return x;
		}
		exp = 1;
		while ((frac & (1ULL << 52)) == 0) {
			frac <<= 1;
			exp--;
		}
		frac &= 0x000fffffffffffffULL;
	}

	e = (int64_t)exp + n;
	if (e >= 0x7ff) {
		v.u = sign | (0x7ffULL << 52);
		return v.d;
	}
	if (e <= 0) {
		if (e <= -52) {
			v.u = sign;
			return v.d;
		}
		frac |= 1ULL << 52;
		frac >>= (1 - e);
		v.u = sign | frac;
		return v.d;
	}
	v.u = sign | ((uint64_t)e << 52) | frac;
	return v.d;
}

long double ldexpl(long double x, int n)
{
	return (long double)ldexp((double)x, n);
}

double scalbn(double x, int n)
{
	return ldexp(x, n);
}

long double scalbnl(long double x, int n)
{
	return ldexpl(x, n);
}

long double strtold(const char *nptr, char **endptr)
{
	return (long double)strtod(nptr, endptr);
}
