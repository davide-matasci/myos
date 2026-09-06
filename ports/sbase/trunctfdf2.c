/* Minimal stubs for newlib printf/scanf on soft-float AArch64 (IEEE-128 long double). */
double __trunctfdf2(long double a)
{
	return (double)a;
}

long double __extenddftf2(double a)
{
	return (long double)a;
}

long double __extendsftf2(float a)
{
	return (long double)a;
}

float __trunctfsf2(long double a)
{
	return (float)a;
}
