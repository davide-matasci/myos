/*
 * The binary128 (TF, `long double`) helpers compiler-rt's builtins lack for
 * riscv64 without the F, D and Q extensions: the conversions newlib's
 * printf, strtod and libm reach for, and the unordered check. Everything
 * else riscv64's soft float needs (double and float arithmetic, compares,
 * integer conversions) is compiler-rt's, built by
 * ports/curl/build-softfloat-riscv64.sh into target/libsoftfloat-riscv64.a,
 * which every riscv64 link takes (myos_riscv64_softfloat in
 * scripts/myos-c-userspace-lib.sh). An earlier version of this file carried
 * hand-written double helpers; they were wrong (80 + 100 gave 116, the
 * compares read two doubles as one long double), see the history.
 */
#include <stdint.h>

typedef __attribute__((mode(TF))) float TFtype;

/*
 * IEEE binary128 <-> binary64/32 conversions.
 *
 * These MUST be implemented with integer bit inspection, NOT casts: the
 * riscv64 target is soft-TF (no Q extension), so `return (double)a` inside
 * __trunctfdf2 made the compiler emit a call to __trunctfdf2 itself —
 * infinite recursion, the user stack ran off its pages and the os-test
 * make stage died with "store page fault ... sepc=<__trunctfdf2+2>".
 * (The TF-compare family below also keeps its (double)a casts; those now
 * terminate in the fixed converters.)
 */
union __myos_tf_bits { TFtype f; unsigned __int128 u; };
union __myos_df_bits { double f; uint64_t u; };

static unsigned __int128 __myos_tf_sig113(uint64_t hi, uint64_t lo)
{
	/* implicit integer bit + 112 stored frac bits */
	return ((unsigned __int128)1 << 112) |
	       (((unsigned __int128)(hi & 0x0000ffffffffffffULL)) << 64) | lo;
}

double __trunctfdf2(TFtype a)
{
	union __myos_tf_bits ua = { a };
	const uint64_t hi = (uint64_t)(ua.u >> 64);
	const uint64_t lo = (uint64_t)ua.u;
	const uint64_t sign = (hi >> 63) & 1;
	const uint32_t sexp = (uint32_t)((hi >> 48) & 0x7fff);
	union __myos_df_bits r;

	if (sexp == 0x7fff) {
		if ((hi & 0x0000ffffffffffffULL) != 0 || lo != 0)
			r.u = (sign << 63) | 0x7ff8000000000000ULL; /* NaN */
		else
			r.u = (sign << 63) | 0x7ff0000000000000ULL; /* inf */
		return r.f;
	}
	if (sexp == 0) {
		/* TF zero, or a TF denormal (~1e-4951) far below double range */
		r.u = sign << 63;
		return r.f;
	}

	unsigned __int128 sig = __myos_tf_sig113(hi, lo) >> 60; /* 53 bits */
	const uint64_t rem = lo & 0x0fffffffffffffffULL;
	int32_t dexp = (int32_t)sexp - 16383 + 1023;

	/* round-to-nearest-even on the 60 dropped bits */
	if ((rem >> 59) & 1)
		sig += (rem & 0x07ffffffffffffffULL) != 0 || (sig & 1);
	if (sig == ((unsigned __int128)1 << 53)) {
		sig >>= 1;
		dexp++;
	}
	if (dexp >= 2047) {
		r.u = (sign << 63) | 0x7ff0000000000000ULL; /* overflow -> inf */
		return r.f;
	}
	if (dexp <= 0) {
		/* double denormal range: shift right with RNE */
		const int32_t shift = 1 - dexp;
		if (shift > 54) {
			r.u = sign << 63;
			return r.f;
		}
		uint64_t m = (uint64_t)sig;
		const uint64_t dropped = m & ((1ULL << shift) - 1);
		m >>= shift;
		if ((dropped >> (shift - 1)) & 1)
			m += (dropped & ((1ULL << (shift - 1)) - 1)) != 0 || (m & 1);
		r.u = (sign << 63) | m;
		return r.f;
	}
	r.u = (sign << 63) | ((uint64_t)dexp << 52) |
	      ((uint64_t)sig & 0x000fffffffffffffULL);
	return r.f;
}

TFtype __extenddftf2(double a)
{
	union __myos_df_bits da = { a };
	const uint64_t sign = (da.u >> 63) & 1;
	const uint32_t dexp = (uint32_t)((da.u >> 52) & 0x7ff);
	uint64_t frac = da.u & 0x000fffffffffffffULL;
	union __myos_tf_bits r;
	uint32_t tfexp;
	unsigned __int128 frac112;

	r.u = (unsigned __int128)sign << 127;
	if (dexp == 0x7ff) {
		if (frac != 0) {
			r.u |= ((unsigned __int128)0x7fff << 112) |
			       ((unsigned __int128)1 << 111); /* quiet NaN */
		} else {
			r.u |= (unsigned __int128)0x7fff << 112; /* inf */
		}
		return r.f;
	}
	if (dexp == 0) {
		if (frac == 0)
			return r.f; /* signed zero */
		/* double denormal: renormalize the 52-bit frac */
		int32_t e = -1022;
		while (!(frac >> 52)) {
			frac <<= 1;
			e--;
		}
		tfexp = (uint32_t)(e - 1023 + 16383);
	} else {
		tfexp = dexp - 1023 + 16383;
	}
	frac112 = (unsigned __int128)frac << 60;
	{
		union __myos_tf_bits out;
		out.u = ((unsigned __int128)sign << 127) |
			((unsigned __int128)tfexp << 112) | (frac112 >> 64);
		return out.f;
	}
}

TFtype __extendsftf2(float a)
{
	union { float f; unsigned int u; } sa = { a };
	const uint64_t sign = (sa.u >> 31) & 1;
	const uint32_t sexp = (sa.u >> 23) & 0xff;
	uint32_t frac = sa.u & 0x007fffff;
	union __myos_tf_bits r;
	uint32_t tfexp;
	unsigned __int128 frac112;

	r.u = (unsigned __int128)sign << 127;
	if (sexp == 0xff) {
		if (frac != 0) {
			r.u |= ((unsigned __int128)0x7fff << 112) |
			       ((unsigned __int128)1 << 111); /* quiet NaN */
		} else {
			r.u |= (unsigned __int128)0x7fff << 112; /* inf */
		}
		return r.f;
	}
	if (sexp == 0) {
		if (frac == 0)
			return r.f; /* signed zero */
		int32_t e = -126;
		while (!(frac >> 23)) {
			frac <<= 1;
			e--;
		}
		tfexp = (uint32_t)(e - 127 + 16383);
	} else {
		tfexp = sexp - 127 + 16383;
	}
	frac112 = (unsigned __int128)frac << (112 - 23);
	{
		union __myos_tf_bits out;
		out.u = ((unsigned __int128)sign << 127) |
			((unsigned __int128)tfexp << 112) | (frac112 >> 64);
		return out.f;
	}
}

float __trunctfsf2(TFtype a)
{
	union __myos_tf_bits ua = { a };
	const uint64_t hi = (uint64_t)(ua.u >> 64);
	const uint64_t lo = (uint64_t)ua.u;
	const uint64_t sign = (hi >> 63) & 1;
	const uint32_t sexp = (uint32_t)((hi >> 48) & 0x7fff);
	union { float f; unsigned int u; } r;

	if (sexp == 0x7fff) {
		if ((hi & 0x0000ffffffffffffULL) != 0 || lo != 0)
			r.u = (unsigned int)(sign << 31) | 0x7fc00000u; /* NaN */
		else
			r.u = (unsigned int)(sign << 31) | 0x7f800000u; /* inf */
		return r.f;
	}
	if (sexp == 0) {
		r.u = (unsigned int)(sign << 31);
		return r.f;
	}

	unsigned __int128 sig = __myos_tf_sig113(hi, lo) >> 89; /* 24 bits */
	const unsigned __int128 rem = ua.u & (((unsigned __int128)1 << 89) - 1);
	int32_t fexp = (int32_t)sexp - 16383 + 127;

	if ((rem >> 88) & 1)
		sig += (rem & (((unsigned __int128)1 << 88) - 1)) != 0 || (sig & 1);
	if (sig == (unsigned __int128)1 << 24) {
		sig >>= 1;
		fexp++;
	}
	if (fexp >= 255) {
		r.u = (unsigned int)(sign << 31) | 0x7f800000u;
		return r.f;
	}
	if (fexp <= 0) {
		const int32_t shift = 1 - fexp;
		if (shift > 25) {
			r.u = (unsigned int)(sign << 31);
			return r.f;
		}
		unsigned int m = (unsigned int)sig;
		const unsigned int dropped = m & ((1u << shift) - 1);
		m >>= shift;
		if ((dropped >> (shift - 1)) & 1)
			m += (dropped & ((1u << (shift - 1)) - 1)) != 0 || (m & 1);
		r.u = (unsigned int)(sign << 31) | m;
		return r.f;
	}
	r.u = (unsigned int)(sign << 31) | ((unsigned int)fexp << 23) |
	      ((unsigned int)sig & 0x007fffff);
	return r.f;
}



/*
 * Double/float helpers for the soft-float riscv64imac target.
 *
 * Everything below is implemented with INTEGER bit inspection and
 * integer arithmetic ONLY. The target has no F/D extensions, so any
 * double/float arithmetic or comparison written in C here made the
 * compiler emit a call back into these very helpers (__eqdf2 called
 * __eqdf2, __trunctfdf2 called __trunctfdf2, ...) — infinite recursion
 * that ran the user stack off its end and killed the os-test make stage
 * with "store page fault stval=... sepc=<__trunctfdf2+2>".
 */


/* total-order key: IEEE-754 bits are totally ordered by flipping the sign
 * bit for positives and inverting all bits for negatives. -0 is normalized
 * to +0 first so the two zeros compare equal. */

/*
 * TF (128-bit) unordered check. Apple clang recognizes the NaN-check idiom
 * in __unorddf2 above ((double)a != (double)a) and canonicalizes it into a
 * direct TF comparison, emitting a call to __unordtf2 — observed on the
 * macOS host; Linux gcc keeps the double comparison, which is why this
 * only links there. Implemented with integer bit inspection (TF is NaN
 * iff the exponent bits are all ones and the mantissa is nonzero) so the
 * compiler never emits a recursive __*tf2 call here.
 */
int __unordtf2(TFtype a, TFtype b)
{
	union { TFtype f; unsigned __int128 u; } ua = { a };
	union { TFtype f; unsigned __int128 u; } ub = { b };
	const unsigned __int128 mant = (~(unsigned __int128)0) >> 15;
	return (((ua.u >> 112) == 0x7fff) && (ua.u & mant) != 0) ||
	       (((ub.u >> 112) == 0x7fff) && (ub.u & mant) != 0);
}
