/*
 * Says what a Bus error hit, in a test that starts Ergo, before the test dies of it: the
 * address, the thread (a Rust test's thread is named after its test) and the line of
 * /proc/self/maps holding that address, such as a SQLite -shm file someone truncated.
 * zirc-server's media and join tests died of SIGBUS on CI's Linux agents only (builds 4158,
 * 4346, 4437, 4588, 4594, 4655), their logs ending mid-line with nothing else to go by.
 *
 * C, as zirc's crates forbid unsafe Rust (//bazel:lints). Installed before main, so
 * Rust's runtime, which sets its own SIGBUS handler only over the default one, leaves it;
 * once it has written, the default comes back and the test dies of the signal as before.
 * Everything in the handler is async-signal-safe: open, read, write, no allocation.
 */
#include <fcntl.h>
#include <signal.h>
#include <stdint.h>
#include <string.h>
#include <unistd.h>

static void say(const char *text, size_t length) {
    while (length > 0) {
        ssize_t written = write(STDERR_FILENO, text, length);
        if (written <= 0) return;
        text += written;
        length -= (size_t)written;
    }
}

static void say_text(const char *text) { say(text, strlen(text)); }

static void say_hex(uintptr_t value) {
    char digits[2 + 2 * sizeof value];
    size_t at = sizeof digits;
    do {
        digits[--at] = "0123456789abcdef"[value & 15];
        value >>= 4;
    } while (value != 0);
    digits[--at] = 'x';
    digits[--at] = '0';
    say(digits + at, sizeof digits - at);
}

/* The hexadecimal number at *text, moving past it. */
static uintptr_t hex(const char **text) {
    uintptr_t value = 0;
    for (;; (*text)++) {
        char c = **text;
        if (c >= '0' && c <= '9') value = value * 16 + (uintptr_t)(c - '0');
        else if (c >= 'a' && c <= 'f') value = value * 16 + (uintptr_t)(c - 'a' + 10);
        else return value;
    }
}

/* Whether a line of /proc/self/maps, "start-end perms offset dev inode path", holds address. */
static int holds(const char *line, uintptr_t address) {
    uintptr_t start = hex(&line);
    if (*line != '-') return 0;
    line++;
    uintptr_t end = hex(&line);
    return address >= start && address < end;
}

static void say_mapping(uintptr_t address) {
    int maps = open("/proc/self/maps", O_RDONLY);
    if (maps < 0) {
        say_text("bus error: /proc/self/maps can't be read\n");
        return;
    }
    char chunk[4096], line[1024];
    size_t length = 0;
    int found = 0;
    ssize_t got;
    while (!found && (got = read(maps, chunk, sizeof chunk)) > 0) {
        for (ssize_t i = 0; i < got && !found; i++) {
            if (chunk[i] != '\n') {
                if (length < sizeof line - 1) line[length++] = chunk[i];
                continue;
            }
            line[length] = '\0';
            if (holds(line, address)) {
                say_text("bus error: in ");
                say(line, length);
                say_text("\n");
                found = 1;
            }
            length = 0;
        }
    }
    close(maps);
    if (!found) say_text("bus error: the address is in no mapping\n");
}

static void on_bus_error(int signal, siginfo_t *info, void *context) {
    (void)signal;
    (void)context;
    uintptr_t address = (uintptr_t)info->si_addr;
    say_text("bus error: SIGBUS at ");
    say_hex(address);
    say_text(", si_code ");
    say_hex((uintptr_t)info->si_code);
    say_text(", thread ");
    int comm = open("/proc/thread-self/comm", O_RDONLY);
    if (comm >= 0) {
        char name[64];
        ssize_t got = read(comm, name, sizeof name);
        if (got > 0) say(name, (size_t)got);
        close(comm);
    } else {
        say_text("?\n");
    }
    say_mapping(address);
    /* SA_RESETHAND has put the default back: returning faults again, and the test dies. */
}

__attribute__((constructor)) static void install(void) {
    struct sigaction action;
    memset(&action, 0, sizeof action);
    action.sa_sigaction = on_bus_error;
    action.sa_flags = SA_SIGINFO | SA_RESETHAND;
    sigemptyset(&action.sa_mask);
    sigaction(SIGBUS, &action, NULL);
}
