/* The generated declarations compile unchanged as C++17. The C++ branch in
 * calculate catches exceptions before returning through the C ABI. */
#include "foreign.h"
#include "plugin.c"
static_assert(sizeof(zp_header) == 16, "frozen protocol header");
