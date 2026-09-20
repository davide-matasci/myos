/* myos_builtins.c — 128-bit integer helpers clang expects from compiler-rt
 * (no libclang_rt.builtins for the myos triples). */
typedef unsigned __int128 u128;
typedef __int128 i128;

u128 __udivti3(u128 a, u128 b) {
	u128 q = 0, r = 0;
	int i;
	if (b == 0) {
		return (u128)0 - 1;
	}
	for (i = 127; i >= 0; i--) {
		r = (r << 1) | ((a >> i) & 1);
		if (r >= b) {
			r -= b;
			q |= (u128)1 << i;
		}
	}
	return q;
}

u128 __umodti3(u128 a, u128 b) {
	u128 q = __udivti3(a, b);
	return a - q * b;
}

u128 __udivmodti4(u128 a, u128 b, u128 *rem) {
	u128 q = __udivti3(a, b);
	if (rem) {
		*rem = a - q * b;
	}
	return q;
}

i128 __divti3(i128 a, i128 b) {
	int neg = (a < 0) != (b < 0);
	u128 ua = a < 0 ? (u128)0 - (u128)a : (u128)a;
	u128 ub = b < 0 ? (u128)0 - (u128)b : (u128)b;
	u128 q = __udivti3(ua, ub);
	return neg ? (i128)((u128)0 - q) : (i128)q;
}

i128 __modti3(i128 a, i128 b) {
	i128 q = __divti3(a, b);
	return a - q * b;
}

i128 __multi3(i128 a, i128 b) {
	return a * b;
}
