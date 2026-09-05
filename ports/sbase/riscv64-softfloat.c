/* Soft-float compiler-rt shims for newlib on RISC-V (no F/D extensions). */
typedef __attribute__((mode(TF))) float TFtype;

double __trunctfdf2(TFtype a)
{
	return (double)a;
}

int __eqdf2(TFtype a, TFtype b)
{
	double da = (double)a;
	double db = (double)b;
	if (da == db) {
		return 0;
	}
	return da > db ? 1 : -1;
}

int __gtdf2(TFtype a, TFtype b)
{
	double da = (double)a;
	double db = (double)b;
	return da > db ? 1 : 0;
}

int __ledf2(TFtype a, TFtype b)
{
	double da = (double)a;
	double db = (double)b;
	return da <= db ? 1 : 0;
}

double __adddf3(double a, double b)
{
	return a + b;
}

double __subdf3(double a, double b)
{
	return a - b;
}

double __muldf3(double a, double b)
{
	return a * b;
}

double __divdf3(double a, double b)
{
	return a / b;
}

double __extendsfdf2(float a)
{
	return (double)a;
}

float __truncdfsf2(double a)
{
	return (float)a;
}

int __fixdfsi(double a)
{
	return (int)a;
}

double __floatsidf(int a)
{
	return (double)a;
}

int __fixunsdfsi(double a)
{
	return (unsigned)a;
}

double __floatunsidf(unsigned a)
{
	return (double)a;
}

int __ltdf2(TFtype a, TFtype b)
{
	double da = (double)a;
	double db = (double)b;
	if (da == db) {
		return 0;
	}
	return da < db ? -1 : 1;
}

int __gedf2(TFtype a, TFtype b)
{
	double da = (double)a;
	double db = (double)b;
	if (da == db) {
		return 0;
	}
	return da > db ? 1 : -1;
}

double __floatdidf(long long a)
{
	return (double)a;
}

double __floatundidf(unsigned long long a)
{
	return (double)a;
}

long long __fixdfdi(double a)
{
	return (long long)a;
}

unsigned long long __fixunsdfdi(double a)
{
	return (unsigned long long)a;
}

int __nedf2(TFtype a, TFtype b)
{
	double da = (double)a;
	double db = (double)b;
	return da != db ? 1 : 0;
}

int __unorddf2(TFtype a, TFtype b)
{
	double da = (double)a;
	double db = (double)b;
	return da != da || db != db ? 1 : 0;
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

static unsigned int __myos_sf_key(float a)
{
	unsigned int x = ((__myos_sf_bits){a}).u;
	return (x & 0x80000000u) ? ~x : (x | 0x80000000u);
}

int __eqsf2(float a, float b)
{
	return __myos_sf_key(a) == __myos_sf_key(b) ? 0 : -1;
}

int __nesf2(float a, float b)
{
	return __myos_sf_key(a) != __myos_sf_key(b) ? 1 : 0;
}

int __ltsf2(float a, float b)
{
	return __myos_sf_key(a) < __myos_sf_key(b) ? 1 : 0;
}

int __gtsf2(float a, float b)
{
	return __myos_sf_key(a) > __myos_sf_key(b) ? 1 : 0;
}

int __lesf2(float a, float b)
{
	return __myos_sf_key(a) <= __myos_sf_key(b) ? 1 : 0;
}

int __gesf2(float a, float b)
{
	return __myos_sf_key(a) >= __myos_sf_key(b) ? 1 : 0;
}
