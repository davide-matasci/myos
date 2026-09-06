#include <unistd.h>

int main(void) {
    const char msg[] = "[ OK ] c\n";
    write(STDOUT_FILENO, msg, sizeof msg - 1);
    return 0;
}
