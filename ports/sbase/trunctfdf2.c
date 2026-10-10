/*
 * The IEEE binary128 (TF, `long double` on aarch64 and riscv64) <->
 * binary64/binary32 conversions newlib's printf, strtod and libm call:
 * __trunctfdf2, __trunctfsf2, __extenddftf2 and __extendsftf2. Linked by
 * every aarch64 C program; riscv64-softfloat.c includes this file for
 * riscv64.
 *
 * Written with integer bit inspection only, never a cast between the
 * types: neither target has quad-precision hardware, so `return
 * (double)a;` here compiles to a call to the function being defined (an
 * endless recursion, issue #374).
 */
#include <stdint.h>

typedef __attribute__((mode(TF))) float TFtype;

union __myos_tf_bits { TFtype f; unsigned __int128 u; };

#define TF_FRAC 112
#define TF_BIAS 16383
#define TF_EXP_MAX 0x7fff

/*
 * `a` rounded to nearest-even in a format with `frac` fraction bits and an
 * exponent of `exp_bits` biased by `bias`, as that format's bits.
 */
static uint64_t __myos_tf_trunc(TFtype a, int frac, int exp_bits, int bias)
{
	union __myos_tf_bits ua = { a };
	const uint64_t sign = (uint64_t)(ua.u >> 127) << (frac + exp_bits);
	const int texp = (int)(ua.u >> TF_FRAC) & TF_EXP_MAX;
	const unsigned __int128 tfrac = ua.u & (((unsigned __int128)1 << TF_FRAC) - 1);
	const uint64_t exp_max = (1ULL << exp_bits) - 1;

	if (texp == TF_EXP_MAX) {
		/* inf, or a quiet NaN */
		return sign | exp_max << frac | (tfrac != 0 ? 1ULL << (frac - 1) : 0);
	}
	if (texp == 0) {
		/* zero, or a TF denormal: far below the narrower format's range */
		return sign;
	}
	/* the significand with its integer bit, and the exponent it gets */
	const unsigned __int128 sig = ((unsigned __int128)1 << TF_FRAC) | tfrac;
	int e = texp - TF_BIAS + bias;
	/* bits to drop: a denormal result drops one more per step below 1 */
	int shift = TF_FRAC - frac + (e < 1 ? 1 - e : 0);
	if (shift > TF_FRAC + 2) {
		/* less than half the smallest denormal */
		return sign;
	}
	uint64_t m = (uint64_t)(sig >> shift);
	const unsigned __int128 rest = sig & (((unsigned __int128)1 << shift) - 1);
	const unsigned __int128 half = (unsigned __int128)1 << (shift - 1);
	if (rest > half || (rest == half && (m & 1))) {
		m++;
	}
	if (e < 1) {
		/* a denormal; rounding up to 1 << frac makes it the smallest normal */
		return sign | m;
	}
	if (m == 1ULL << (frac + 1)) {
		m >>= 1;
		e++;
	}
	if ((uint64_t)e >= exp_max) {
		return sign | exp_max << frac;
	}
	return sign | (uint64_t)e << frac | (m & ((1ULL << frac) - 1));
}

/* The format of `bits` (`frac` fraction bits, `exp_bits` of exponent biased
 * by `bias`) widened to TF: exact. */
static TFtype __myos_tf_extend(uint64_t bits, int frac, int exp_bits, int bias)
{
	const uint64_t exp_max = (1ULL << exp_bits) - 1;
	const unsigned __int128 sign = (unsigned __int128)(bits >> (frac + exp_bits) & 1) << 127;
	const uint64_t exp = bits >> frac & exp_max;
	uint64_t f = bits & ((1ULL << frac) - 1);
	union __myos_tf_bits r;
	int e;

	if (exp == exp_max) {
		/* inf, or a quiet NaN */
		r.u = sign | (unsigned __int128)TF_EXP_MAX << TF_FRAC
		    | (f != 0 ? (unsigned __int128)1 << (TF_FRAC - 1) : 0);
		return r.f;
	}
	if (exp == 0) {
		if (f == 0) {
			r.u = sign;
			return r.f;
		}
		/* a denormal: normalize it, TF's range is wide enough */
		e = 1 - bias;
		while (!(f >> frac)) {
			f <<= 1;
			e--;
		}
		f &= (1ULL << frac) - 1;
	} else {
		e = (int)exp - bias;
	}
	r.u = sign | (unsigned __int128)(e + TF_BIAS) << TF_FRAC
	    | (unsigned __int128)f << (TF_FRAC - frac);
	return r.f;
}

double __trunctfdf2(TFtype a)
{
	union { uint64_t u; double f; } r = { __myos_tf_trunc(a, 52, 11, 1023) };
	return r.f;
}

float __trunctfsf2(TFtype a)
{
	union { uint32_t u; float f; } r = { (uint32_t)__myos_tf_trunc(a, 23, 8, 127) };
	return r.f;
}

TFtype __extenddftf2(double a)
{
	union { double f; uint64_t u; } da = { a };
	return __myos_tf_extend(da.u, 52, 11, 1023);
}

TFtype __extendsftf2(float a)
{
	union { float f; uint32_t u; } sa = { a };
	return __myos_tf_extend(sa.u, 23, 8, 127);
}
