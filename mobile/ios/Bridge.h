#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
bool spacemosaic_deliver(const uint8_t *bytes, size_t length);
void spacemosaic_run(void);

uint64_t spacemosaic_begin(void);
bool spacemosaic_is_current(uint64_t generation);

char *spacemosaic_result_page(const char *request);
void spacemosaic_free_result(char *response);
void spacemosaic_export_done(uint64_t id, int32_t outcome);
