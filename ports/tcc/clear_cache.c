/* myos: __clear_cache for libtcc1 when compiling TinyCC lib/ with clang.
 *
 * Upstream lib/armflush.c on aarch64/riscv calls tcc-internal
 * __arm64_clear_cache / __riscv64_clear_cache (only exist if tcc compiles
 * libtcc1 with itself). Use the clang/gcc builtin instead.
 */
void __clear_cache(void *beg, void *end)
{
    __builtin___clear_cache(beg, end);
}
