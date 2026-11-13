/* Input to Zig's C translation tool. */
#include "foreign.h"

/* Translate only the plugin ABI and the four C library functions this example
 * uses. Pulling the complete platform stdlib/string headers into translation is
 * unnecessary and can make Apple SDK translation stall. These are the standard
 * C prototypes; the dynamic plugin still links the platform C library. */
void *malloc(size_t size);
void *calloc(size_t count, size_t size);
void free(void *pointer);
void *memcpy(void *destination, const void *source, size_t size);
