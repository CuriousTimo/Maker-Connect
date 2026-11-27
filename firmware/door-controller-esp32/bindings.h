/* ESP-IDF items esp-idf-sys does not bind by default: flash encryption on, and the eFuse table for
 * Secure Boot v2 (esp_secure_boot_enabled() is static inline; on the ESP32 it reads ABS_DONE_1). */
#include "esp_efuse.h"
#include "esp_efuse_table.h"
#include "esp_flash_encrypt.h"
