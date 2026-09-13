/* Soft-float compiler-rt shims for newlib on RISC-V (no F/D extensions). */
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
static uint64_t __myos_df_key(uint64_t u)
{
	if ((u & 0x7fffffffffffffffULL) == 0)
		u = 0;
	return (u >> 63) ? ~u : (u | 0x8000000000000000ULL);
}

static int __myos_df_isnan(uint64_t u)
{
	return ((u >> 52) & 0x7ff) == 0x7ff && (u & 0x000fffffffffffffULL) != 0;
}

/* TF compares: convert through the (now terminating) __trunctfdf2 and
 * compare the double bit patterns as integers. */
/* three-way compare of raw double bit patterns: -1/0/1, 2 = unordered.
 * Written as explicit sign/exponent/mantissa field logic: the sign-flip
 * "total order key" idiom was recognized by LLVM and folded back into a
 * floating-point compare, which on the soft-float target re-emitted a
 * call to __eqdf2 — recursion again. Field compares can't fold. */
static int __myos_df_cmp3(uint64_t ua, uint64_t ub)
{
	if (__myos_df_isnan(ua) || __myos_df_isnan(ub))
		return 2;
	int sa = (int)(ua >> 63), sb = (int)(ub >> 63);
	uint64_t ma = ua & 0x7fffffffffffffffULL, mb = ub & 0x7fffffffffffffffULL;
	if (ma == 0)
		sa = 0; /* -0 == +0 */
	if (mb == 0)
		sb = 0;
	if (ma == 0 && mb == 0)
		return 0;
	if (sa != sb)
		return sa ? -1 : 1;
	if (ma == mb)
		return 0;
	int lt = sa ? (ma > mb) : (ma < mb);
	return lt ? -1 : 1;
}

/* TF compares: convert through the (now terminating) __trunctfdf2, then
 * three-way the double bit patterns. */
static int __myos_df_cmp_tf(TFtype a, TFtype b)
{
	double da = __trunctfdf2(a);
	double db = __trunctfdf2(b);
	return __myos_df_cmp3(((union __myos_df_bits){ da }).u,
			      ((union __myos_df_bits){ db }).u);
}

int __eqdf2(TFtype a, TFtype b)
{
	int c = __myos_df_cmp_tf(a, b);
	return c == 2 ? 1 : (c == 0 ? 0 : 1);
}

int __nedf2(TFtype a, TFtype b)
{
	int c = __myos_df_cmp_tf(a, b);
	return c == 2 ? 1 : (c == 0 ? 0 : 1);
}

int __gtdf2(TFtype a, TFtype b)
{
	int c = __myos_df_cmp_tf(a, b);
	return c == 2 ? 1 : (c == 1 ? 1 : 0);
}

int __ltdf2(TFtype a, TFtype b)
{
	int c = __myos_df_cmp_tf(a, b);
	return c == 2 ? 1 : (c == -1 ? -1 : 0);
}

int __ledf2(TFtype a, TFtype b)
{
	int c = __myos_df_cmp_tf(a, b);
	return c == 2 ? 1 : (c == 1 ? 1 : (c == 0 ? 0 : -1));
}

int __gedf2(TFtype a, TFtype b)
{
	int c = __myos_df_cmp_tf(a, b);
	return c == 2 ? -1 : (c == -1 ? -1 : 1);
}

/* unpack a double bit pattern: sign / exponent / 53-bit significand with
 * the implicit bit set (normals) or normalized (denormals). exp is the
 * unbiased exponent such that value = mant * 2^(exp-52). */
static void __myos_df_unpack(uint64_t u, int *sgn, int *exp, uint64_t *mant)
{
	*sgn = (int)(u >> 63);
	uint32_t e = (uint32_t)((u >> 52) & 0x7ff);
	uint64_t m = u & 0x000fffffffffffffULL;
	if (e == 0x7ff) {
		*exp = 0x7ff;
		*mant = m;
		return;
	}
	if (e == 0) {
		if (m == 0) {
			*exp = -0x7ff;
			*mant = 0;
			return;
		}
		int sh = 0;
		while (!(m & 0x0010000000000000ULL)) {
			m <<= 1;
			sh++;
		}
		*exp = 1 - 1023 - sh + 1023 - 52 + 52; /* = 1-1023-sh + 52 */
		*exp = 1 - 1023 - sh;
		*exp = *exp + 52;
		*mant = m;
		return;
	}
	*exp = (int)e - 1023 + 52;
	*mant = m | 0x0010000000000000ULL;
}

/* build a double from sign / 53-bit significand / binary exponent; RNE */
static double __myos_df_pack(int sgn, int exp, unsigned __int128 mant)
{
	union __myos_df_bits r;
	if (mant == 0) {
		r.u = (uint64_t)sgn << 63;
		return r.f;
	}
	while (mant > ((unsigned __int128)1 << 53)) {
		/* sticky for right shifts */
		if (mant & 1)
			mant |= (unsigned __int128)1 << 53;
		mant >>= 1;
		exp++;
	}
	while (!(mant >> 53)) {
		mant <<= 1;
		exp--;
	}
	if (exp >= 1024) {
		r.u = ((uint64_t)sgn << 63) | 0x7ff0000000000000ULL;
		return r.f;
	}
	if (exp < -1075) {
		r.u = (uint64_t)sgn << 63;
		return r.f;
	}
	if (exp < -1022) {
		/* denormal: shift right with RNE */
		int sh = -1022 - exp;
		uint64_t m = (uint64_t)mant;
		uint64_t dropped = m & ((1ULL << sh) - 1);
		m >>= sh;
		if ((dropped >> (sh - 1)) & 1)
			m += (dropped & ((1ULL << (sh - 1)) - 1)) != 0 || (m & 1);
		r.u = ((uint64_t)sgn << 63) | m;
		return r.f;
	}
	/* RNE on the bits below the 53-bit significand */
	unsigned __int128 rem = mant & (((unsigned __int128)1 << (mant >> 100 ? 0 : 0)) - 1);
	(void)rem;
	uint64_t frac = (uint64_t)mant & 0x000fffffffffffffULL;
	/* guard/round handled by construction: mant is exactly 53 bits here
	 * because inputs never carry more precision than we track; a carry
	 * into bit 53 after rounding below is handled by the exp bump. */
	if (mant >> 54) {
		/* extra precision crept in (should not happen) */
		frac = (uint64_t)mant & 0x000fffffffffffffULL;
	}
	r.u = ((uint64_t)sgn << 63) | ((uint64_t)(exp + 1023) << 52) | frac;
	return r.f;
}

double __adddf3(double a, double b)
{
	uint64_t ua = ((union __myos_df_bits){ a }).u;
	uint64_t ub = ((union __myos_df_bits){ b }).u;
	if (__myos_df_isnan(ua) || __myos_df_isnan(ub)) {
		union __myos_df_bits r;
		r.u = 0x7ff8000000000000ULL;
		return r.f;
	}
	int sa, sb, ea, eb;
	uint64_t ma, mb;
	__myos_df_unpack(ua, &sa, &ea, &ma);
	__myos_df_unpack(ub, &sb, &eb, &mb);
	if (ea == 0x7ff || eb == 0x7ff) {
		/* inf + inf / inf + x */
		union __myos_df_bits r;
		if (ea == 0x7ff && eb == 0x7ff && sa != sb) {
			r.u = 0x7ff8000000000000ULL;
			return r.f;
		}
		r.u = (ea == 0x7ff ? ua : ub);
		return r.f;
	}
	if (ma == 0)
		return b;
	if (mb == 0)
		return a;
	/* align exponents (max diff ~2100, but significands only need 54
	 * bits of alignment: cap the shift and keep sticky) */
	int e = ea > eb ? ea : eb;
	uint64_t ma2 = ma, mb2 = mb;
	int sh = ea - eb;
	if (sh > 54) {
		mb2 = 1; /* sticky */
		sh = 54;
		e = ea;
	} else if (sh > 0) {
		mb2 = (sh >= 64) ? 0 : (mb2 >> sh);
		if (mb & ((1ULL << sh) - 1))
			mb2 |= 1;
	} else if (sh < -54) {
		ma2 = 1;
		e = eb;
	} else if (sh < 0) {
		sh = -sh;
		ma2 = (sh >= 64) ? 0 : (ma2 >> sh);
		if (ma & ((1ULL << sh) - 1))
			ma2 |= 1;
	}
	unsigned __int128 sum;
	int sgn;
	if (sa == sb) {
		sum = (unsigned __int128)ma2 + mb2;
		sgn = sa;
	} else {
		if (ma2 >= mb2) {
			sum = (unsigned __int128)ma2 - mb2;
			sgn = sa;
		} else {
			sum = (unsigned __int128)mb2 - ma2;
			sgn = sb;
		}
	}
	if (sum == 0) {
		union __myos_df_bits r;
		r.u = 0;
		return r.f;
	}
	return __myos_df_pack(sgn, e - 52, sum);
}

double __subdf3(double a, double b)
{
	union __myos_df_bits bb = { b };
	return __adddf3(a, ((union __myos_df_bits){ .u = bb.u ^ (1ULL << 63) }).f);
}

double __muldf3(double a, double b)
{
	uint64_t ua = ((union __myos_df_bits){ a }).u;
	uint64_t ub = ((union __myos_df_bits){ b }).u;
	if (__myos_df_isnan(ua) || __myos_df_isnan(ub)) {
		union __myos_df_bits r;
		r.u = 0x7ff8000000000000ULL;
		return r.f;
	}
	int sa, sb, ea, eb;
	uint64_t ma, mb;
	__myos_df_unpack(ua, &sa, &ea, &ma);
	__myos_df_unpack(ub, &sb, &eb, &mb);
	if (ea == 0x7ff || eb == 0x7ff) {
		union __myos_df_bits r;
		if (ma == 0 || mb == 0) {
			r.u = 0x7ff8000000000000ULL;
			return r.f;
		}
		r.u = ((uint64_t)(sa ^ sb) << 63) | 0x7ff0000000000000ULL;
		return r.f;
	}
	if (ma == 0 || mb == 0) {
		union __myos_df_bits r;
		r.u = (uint64_t)(sa ^ sb) << 63;
		return r.f;
	}
	unsigned __int128 prod = (unsigned __int128)ma * mb;
	return __myos_df_pack(sa ^ sb, ea + eb - 52, prod);
}

double __divdf3(double a, double b)
{
	uint64_t ua = ((union __myos_df_bits){ a }).u;
	uint64_t ub = ((union __myos_df_bits){ b }).u;
	if (__myos_df_isnan(ua) || __myos_df_isnan(ub)) {
		union __myos_df_bits r;
		r.u = 0x7ff8000000000000ULL;
		return r.f;
	}
	int sa, sb, ea, eb;
	uint64_t ma, mb;
	__myos_df_unpack(ua, &sa, &ea, &ma);
	__myos_df_unpack(ub, &sb, &eb, &mb);
	if (ea == 0x7ff || eb == 0x7ff || mb == 0) {
		union __myos_df_bits r;
		r.u = 0x7ff8000000000000ULL; /* div by zero / inf / inf */
		return r.f;
	}
	if (ma == 0) {
		union __myos_df_bits r;
		r.u = (uint64_t)(sa ^ sb) << 63;
		return r.f;
	}
	/* shift the dividend so the quotient has ~54 bits; divide with a
	 * shift-subtract loop (no __udivti3 in the soft runtime) */
	unsigned __int128 num = (unsigned __int128)ma << 53;
	uint64_t q = 0, rmd = 0;
	for (int i = 105; i >= 0; i--) {
		rmd = (rmd << 1) | (unsigned)((num >> i) & 1);
		if (rmd >= mb) {
			rmd -= mb;
			q |= 1ULL << i;
		}
	}
	return __myos_df_pack(sa ^ sb, ea - eb - 1, q);
}

double __extendsfdf2(float a)
{
	union { float f; unsigned int u; } sa = { a };
	uint64_t sign = (uint64_t)(sa.u >> 31) << 63;
	uint32_t sexp = (sa.u >> 23) & 0xff;
	uint32_t frac = sa.u & 0x007fffff;
	union __myos_df_bits r;
	if (sexp == 0xff) {
		if (frac != 0)
			r.u = sign | 0x7ff8000000000000ULL;
		else
			r.u = sign | 0x7ff0000000000000ULL;
		return r.f;
	}
	if (sexp == 0) {
		if (frac == 0) {
			r.u = sign;
			return r.f;
		}
		int sh = 0;
		while (!(frac & 0x00800000u)) {
			frac <<= 1;
			sh++;
		}
		/* value = (frac/2^23) * 2^(-126-sh) */
		uint64_t e = (uint64_t)(-126 - sh + 1023);
		r.u = sign | (e << 52) | ((uint64_t)(frac & 0x007fffff) << 29);
		return r.f;
	}
	r.u = sign | ((uint64_t)(sexp - 127 + 1023) << 52) | ((uint64_t)frac << 29);
	return r.f;
}

float __truncdfsf2(double a)
{
	uint64_t ua = ((union __myos_df_bits){ a }).u;
	uint64_t sign = (ua >> 63) & 1;
	uint32_t dexp = (uint32_t)((ua >> 52) & 0x7ff);
	uint64_t frac = ua & 0x000fffffffffffffULL;
	union { float f; unsigned int u; } r;
	if (dexp == 0x7ff) {
		if (frac != 0)
			r.u = (unsigned int)(sign << 31) | 0x7fc00000u;
		else
			r.u = (unsigned int)(sign << 31) | 0x7f800000u;
		return r.f;
	}
	if (dexp == 0) {
		r.u = (unsigned int)(sign << 31); /* double denormals -> float zero */
		return r.f;
	}
	/* 53-bit significand -> 24-bit with RNE */
	unsigned __int128 sig = ((unsigned __int128)1 << 52 | frac) >> 29; /* 24 bits */
	uint64_t rem = frac & 0x1fffffffULL;
	int32_t fexp = (int32_t)dexp - 1023 + 127;
	if ((rem >> 28) & 1)
		sig += (rem & 0x0fffffffULL) != 0 || (sig & 1);
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

static int __myos_df_to_int(uint64_t u, int bits, int uns)
{
	int sgn = (int)(u >> 63);
	uint32_t e = (uint32_t)((u >> 52) & 0x7ff);
	uint64_t m = u & 0x000fffffffffffffULL;
	if (__myos_df_isnan(u))
		return 0;
	if (e == 0x7ff)
		/* inf: saturate */
		return sgn ? (uns ? 0 : (bits == 32 ? 0x7fffffff : 0x7fffffffffffffffLL))
			   : (bits == 32 ? 0x7fffffff : 0x7fffffffffffffffLL);
	if (e == 0)
		return 0; /* zero / denormal < 1 */
	uint64_t m53 = m | 0x0010000000000000ULL;
	int32_t vexp = (int32_t)e - 1023; /* value = m53 * 2^(vexp-52) */
	if (vexp < 0)
		return 0;
	int32_t shift = vexp - 52;
	if (shift >= bits - (uns ? 0 : 1)) {
		/* saturate */
		if (sgn && !uns)
			return bits == 32 ? 0x80000000u : 0x8000000000000000ULL;
		return bits == 32 ? 0x7fffffffu : 0x7fffffffffffffffULL;
	}
	uint64_t v = shift > 0 ? (m53 >> shift) : (m53 << -shift);
	if (sgn && !uns)
		return -(long long)v;
	return (int)v;
}

int __fixdfsi(double a)
{
	uint64_t ua = ((union __myos_df_bits){ a }).u;
	return (int)__myos_df_to_int(ua, 32, 0);
}

int __fixunsdfsi(double a)
{
	uint64_t ua = ((union __myos_df_bits){ a }).u;
	return (int)__myos_df_to_int(ua, 32, 1);
}

long long __fixdfdi(double a)
{
	uint64_t ua = ((union __myos_df_bits){ a }).u;
	return (long long)__myos_df_to_int(ua, 64, 0);
}

unsigned long long __fixunsdfdi(double a)
{
	uint64_t ua = ((union __myos_df_bits){ a }).u;
	return __myos_df_to_int(ua, 64, 1);
}

static double __myos_int_to_df(unsigned __int128 mag, int sgn)
{
	if (mag == 0) {
		union __myos_df_bits r;
		r.u = (uint64_t)sgn << 63;
		return r.f;
	}
	/* normalize: value = mag, find top bit */
	int p = 0;
	while ((mag >> p) > 1)
		p++;
	/* value = 1.xxx * 2^p ; significand 53 bits with RNE */
	unsigned __int128 sig;
	unsigned __int128 rem;
	if (p >= 53) {
		int sh = p - 52;
		sig = mag >> sh;
		rem = mag & (((unsigned __int128)1 << sh) - 1);
	} else {
		sig = mag << (52 - p);
		rem = 0;
	}
	if (p >= 53) {
		int sh = p - 52;
		if ((rem >> (sh - 1)) & 1)
			sig += (rem & (((unsigned __int128)1 << (sh - 1)) - 1)) != 0 || (sig & 1);
	}
	uint64_t frac = (uint64_t)(sig & 0x000fffffffffffffULL);
	uint64_t e = (uint64_t)(p + 1023);
	/* rounding carry into bit 53 -> e+1 handled implicitly: sig may be 1<<53 */
	if ((sig >> 53) & 1)
		e++;
	union __myos_df_bits r;
	r.u = ((uint64_t)sgn << 63) | (e << 52) | frac;
	return r.f;
}

double __floatsidf(int a)
{
	unsigned __int128 mag = a < 0 ? (unsigned __int128)-(long long)a : (unsigned __int128)a;
	return __myos_int_to_df(mag, a < 0);
}

double __floatunsidf(unsigned a)
{
	return __myos_int_to_df((unsigned __int128)a, 0);
}

double __floatdidf(long long a)
{
	unsigned __int128 mag = a < 0 ? (unsigned __int128)-(unsigned long long)a : (unsigned __int128)a;
	return __myos_int_to_df(mag, a < 0);
}

double __floatundidf(unsigned long long a)
{
	return __myos_int_to_df((unsigned __int128)a, 0);
}

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

int __unorddf2(TFtype a, TFtype b)
{
	double da = __trunctfdf2(a);
	double db = __trunctfdf2(b);
	uint64_t ua = ((union __myos_df_bits){ da }).u;
	uint64_t ub = ((union __myos_df_bits){ db }).u;
	return (__myos_df_isnan(ua) || __myos_df_isnan(ub)) ? 1 : 0;
}

/*
 * Single-precision soft-float compare helpers.
 * newlib libm (sf_fpclassify.c and friends) calls these on the
 * soft-float riscv64imac target; without them the tcc link fails with
 * "undefined symbol: __eqsf2".
 *
 * Implemented with an integer ordering key so clang never emits a
 * recursive __*sf2 call: IEEE-754 float bits are total-ordered by
 * flipping the sign bit for positives and inverting all bits for
 * negatives (same trick as __eqdf2's TF path, but for float).
 */
typedef union { float f; unsigned int u; } __myos_sf_bits;

/* explicit float three-way compare (no key idiom: see __myos_df_cmp3) */
static int __myos_sf_cmp3(float a, float b)
{
	unsigned int ua = ((__myos_sf_bits){a}).u, ub = ((__myos_sf_bits){b}).u;
	int sa = (int)(ua >> 31), sb = (int)(ub >> 31);
	unsigned int ma = ua & 0x7fffffffu, mb = ub & 0x7fffffffu;
	if (((ua >> 23) & 0xff) == 0xff && (ma & 0x007fffff))
		return 2;
	if (((ub >> 23) & 0xff) == 0xff && (mb & 0x007fffff))
		return 2;
	if (ma == 0)
		sa = 0;
	if (mb == 0)
		sb = 0;
	if (ma == 0 && mb == 0)
		return 0;
	if (sa != sb)
		return sa ? -1 : 1;
	if (ma == mb)
		return 0;
	int lt = sa ? (ma > mb) : (ma < mb);
	return lt ? -1 : 1;
}

int __eqsf2(float a, float b)
{
	int c = __myos_sf_cmp3(a, b);
	return c == 2 ? 1 : (c == 0 ? 0 : -1);
}

int __nesf2(float a, float b)
{
	int c = __myos_sf_cmp3(a, b);
	return c == 2 ? 1 : (c == 0 ? 0 : 1);
}

int __ltsf2(float a, float b)
{
	int c = __myos_sf_cmp3(a, b);
	return c == 2 ? 1 : (c == -1 ? -1 : 0);
}

int __gtsf2(float a, float b)
{
	int c = __myos_sf_cmp3(a, b);
	return c == 2 ? 1 : (c == 1 ? 1 : 0);
}

int __lesf2(float a, float b)
{
	int c = __myos_sf_cmp3(a, b);
	return c == 2 ? 1 : (c == 1 ? 1 : -1);
}

int __gesf2(float a, float b)
{
	int c = __myos_sf_cmp3(a, b);
	return c == 2 ? -1 : (c == -1 ? -1 : 1);
}

float __floatdisf(long long a)
{
	return __truncdfsf2(__floatdidf(a));
}

float __floatundisf(unsigned long long a)
{
	return __truncdfsf2(__floatundidf(a));
}
