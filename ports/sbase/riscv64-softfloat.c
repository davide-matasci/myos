/*
 * The binary128 (TF, `long double`) helpers compiler-rt's builtins lack for
 * riscv64 without the F, D and Q extensions: the conversions newlib's
 * printf, strtod and libm reach for (trunctfdf2.c, shared with aarch64),
 * and the unordered check. Everything else riscv64's soft float needs
 * (double and float arithmetic, compares, integer conversions) is
 * compiler-rt's, built by ports/curl/build-softfloat-riscv64.sh into
 * target/libsoftfloat-riscv64.a, which every riscv64 link takes
 * (myos_riscv64_softfloat in scripts/myos-c-userspace-lib.sh). An earlier
 * version of this file carried hand-written double helpers; they were
 * wrong (80 + 100 gave 116, the compares read two doubles as one long
 * double), see the history.
 */
#include "trunctfdf2.c"

/*
 * TF (128-bit) unordered check. Apple clang recognizes the NaN-check idiom
 * in a __unorddf2 ((double)a != (double)a) and canonicalizes it into a
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
