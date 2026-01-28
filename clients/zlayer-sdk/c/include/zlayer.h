#ifndef ZLAYER_SDK_H
#define ZLAYER_SDK_H

#include <stdint.h>
#include <stdbool.h>
#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

// Version
#define ZLAYER_SDK_VERSION "0.1.0"

// Config functions
char* zlayer_config_get(const char* key);
char* zlayer_config_get_all(void);
void zlayer_config_free(char* str);

// Key-Value functions
uint8_t* zlayer_kv_get(const char* bucket, const char* key, size_t* out_len);
bool zlayer_kv_set(const char* bucket, const char* key, const uint8_t* value, size_t len);
bool zlayer_kv_delete(const char* bucket, const char* key);
char** zlayer_kv_keys(const char* bucket, const char* prefix, size_t* out_count);
void zlayer_kv_free(uint8_t* data);
void zlayer_kv_free_keys(char** keys, size_t count);

// Logging functions
typedef enum {
    ZLAYER_LOG_TRACE = 0,
    ZLAYER_LOG_DEBUG = 1,
    ZLAYER_LOG_INFO = 2,
    ZLAYER_LOG_WARN = 3,
    ZLAYER_LOG_ERROR = 4
} zlayer_log_level_t;

void zlayer_log(zlayer_log_level_t level, const char* message);
void zlayer_log_trace(const char* message);
void zlayer_log_debug(const char* message);
void zlayer_log_info(const char* message);
void zlayer_log_warn(const char* message);
void zlayer_log_error(const char* message);

// Secrets functions
char* zlayer_secret_get(const char* name);
void zlayer_secret_free(char* str);

// Metrics functions
void zlayer_counter_inc(const char* name, uint64_t value);
void zlayer_gauge_set(const char* name, double value);
void zlayer_histogram_observe(const char* name, double value);

#ifdef __cplusplus
}
#endif

#endif // ZLAYER_SDK_H
