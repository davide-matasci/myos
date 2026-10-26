# tcc, installed as an app (run-myos binds its /bin/tcc and /lib/newlib):
# a program it builds against newlib runs. Its own linker bounds
# .init_array and .fini_array, and it links crti.o and crtn.o: main
# returns 3, and the destructor turns it into 0 if the constructor ran.
ctor_tcc() {
	cat > /tmp/ctor.c <<'EOF'
#include <unistd.h>
static int ran;
__attribute__((constructor)) static void ctor(void) { ran = 1; }
__attribute__((destructor)) static void dtor(void) { if (ran == 2) _exit(0); }
int main(void) { if (ran == 1) ran = 2; return 3; }
EOF
	run-myos tcc -o /tmp/ctor /tmp/ctor.c && /tmp/ctor
}
t -t exec ctor_tcc ctor_tcc
