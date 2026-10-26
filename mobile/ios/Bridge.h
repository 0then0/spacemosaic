#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
bool spacemosaic_deliver(const uint8_t *bytes, size_t length);
void spacemosaic_run(void);

uint64_t spacemosaic_begin(void);
bool spacemosaic_is_current(uint64_t generation);
