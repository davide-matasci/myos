/* Single-precision soft-float arith via double helpers (avoid recursive __*sf3). */
extern double __extendsfdf2(float a);
extern float __truncdfsf2(double a);
extern double __adddf3(double a, double b);
extern double __subdf3(double a, double b);
extern double __muldf3(double a, double b);
extern double __divdf3(double a, double b);
extern double __floatsidf(int a);
extern double __floatunsidf(unsigned a);
extern int __fixdfsi(double a);

float __addsf3(float a, float b) {
    return __truncdfsf2(__adddf3(__extendsfdf2(a), __extendsfdf2(b)));
}
float __subsf3(float a, float b) {
    return __truncdfsf2(__subdf3(__extendsfdf2(a), __extendsfdf2(b)));
}
float __mulsf3(float a, float b) {
    return __truncdfsf2(__muldf3(__extendsfdf2(a), __extendsfdf2(b)));
}
float __divsf3(float a, float b) {
    return __truncdfsf2(__divdf3(__extendsfdf2(a), __extendsfdf2(b)));
}
float __floatsisf(int a) {
    return __truncdfsf2(__floatsidf(a));
}
float __floatunsisf(unsigned a) {
    return __truncdfsf2(__floatunsidf(a));
}
int __fixsfsi(float a) {
    return __fixdfsi(__extendsfdf2(a));
}
