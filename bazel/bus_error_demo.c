/* Dies of a Bus error as the tests did, for bus_error_test.sh: maps a page of a file, truncates
 * the file under it and touches the page, which no longer has a file behind it. */
#include <fcntl.h>
#include <stdio.h>
#include <sys/mman.h>
#include <unistd.h>

int main(int argc, char **argv) {
    if (argc != 2) return 2;
    int file = open(argv[1], O_RDWR | O_CREAT | O_TRUNC, 0600);
    if (file < 0 || ftruncate(file, 4096) != 0) return 2;
    volatile char *page = mmap(NULL, 4096, PROT_READ | PROT_WRITE, MAP_SHARED, file, 0);
    if (page == MAP_FAILED || ftruncate(file, 0) != 0) return 2;
    page[0] = 1;
    puts("no bus error");
    return 0;
}
