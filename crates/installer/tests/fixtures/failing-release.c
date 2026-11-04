/* Acceptance only: a checksum-valid release that passes version inspection but
 * fails startup. Never package this executable as a production component. */
#include <stdio.h>
#include <string.h>

int main(int argc, char **argv) {
    if (argc == 2 && strcmp(argv[1], "--version") == 0) {
        const char *name = strrchr(argv[0], '/');
        printf("%s 0.1.1\n", name ? name + 1 : argv[0]);
        return 0;
    }
    fputs("Intentional acceptance startup failure\n", stderr);
    return 42;
}
