#include <stdint.h>
#include <stdlib.h>
#include <string.h>

typedef struct { const uint8_t *data; size_t len; } slice_t;
typedef struct { uint8_t *data; size_t len; } buffer_t;
typedef struct { uint32_t size; uint32_t operation; slice_t source; const slice_t *arguments; size_t argument_count; int32_t info_flags; uint32_t reserved[8]; } cli_t;
typedef struct { uint32_t size; uint32_t reserved0; slice_t method, uri, query, headers, body, root, script, server, remote; uint16_t server_port, remote_port; uint32_t reserved[8]; } request_t;
typedef struct { uint32_t size; uint16_t status, reserved0; buffer_t headers, body; uint32_t reserved[8]; } response_t;
typedef struct { uint32_t size, reserved0; void *userdata; int32_t (*wait)(void *, request_t *); void (*complete)(void *, const response_t *); uint32_t reserved[8]; } callbacks_t;

typedef struct api_t {
    uint32_t size;
    uint16_t major, minor;
    uint64_t features;
    int32_t (*metadata)(buffer_t *);
    int32_t (*last_error)(buffer_t *);
    void (*free_buffer)(buffer_t *);
    int32_t (*set_ini)(slice_t);
    int32_t (*cli)(const cli_t *, int32_t *);
    int32_t (*web_create)(void **);
    int32_t (*web_execute)(void *, const request_t *, response_t *, int32_t *);
    void (*web_destroy)(void *);
    int32_t (*worker_create)(void **);
    int32_t (*worker_run)(void *, slice_t, slice_t, const callbacks_t *, int32_t *);
    void (*worker_destroy)(void *);
    void *reserved[16];
} api_t;

static int32_t copy(const char *value, buffer_t *output) {
    output->len = strlen(value);
    output->data = malloc(output->len);
    memcpy(output->data, value, output->len);
    return 0;
}

static int32_t metadata(buffer_t *output) {
    return copy("{\"php_version\":\"8.5.9\",\"php_version_id\":80509,\"zend_version\":\"4.5.9\",\"zts\":true,\"debug\":false,\"runtime_revision\":\"fake\",\"target\":\"" POX_TEST_TARGET "\",\"abi_major\":1,\"abi_minor\":0,\"extensions\":[\"Core\"],\"libraries\":{}}", output);
}
static int32_t last_error(buffer_t *output) { return copy("fake error", output); }
static void free_buffer(buffer_t *buffer) { free(buffer->data); buffer->data = NULL; buffer->len = 0; }
static int32_t set_ini(slice_t value) { (void)value; return 0; }
static int32_t cli(const cli_t *request, int32_t *exit_code) { (void)request; *exit_code = 0; return 0; }
static int32_t create(void **runtime) { *runtime = malloc(1); return *runtime ? 0 : 4; }
static int32_t web_execute(void *runtime, const request_t *request, response_t *response, int32_t *exit_code) { (void)runtime; (void)request; response->status = 204; *exit_code = 0; return 0; }
static void destroy(void *runtime) { free(runtime); }
static int32_t worker_run(void *runtime, slice_t script, slice_t root, const callbacks_t *callbacks, int32_t *exit_code) { (void)runtime; (void)script; (void)root; (void)callbacks; *exit_code = 0; return 0; }

static const api_t API = {
    sizeof(api_t), 1, 0, 0,
    metadata, last_error, free_buffer, set_ini, cli,
    create, web_execute, destroy,
    create, worker_run, destroy,
    {0}
};

__attribute__((visibility("default")))
const api_t *pox_php_get_api(uint32_t major, uint32_t minor) {
    return major == 1 && minor == 0 ? &API : NULL;
}
