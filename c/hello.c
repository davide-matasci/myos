#include <unistd.h>

int main(void) {
    const char msg[] = "c ok\n";
    write(STDOUT_FILENO, msg, sizeof msg - 1);
    return 0;
}
