/*
 * phpx Stable ABI Layer
 *
 * This header defines a stable ABI for the PHP runtime library.
 * The main phpx binary dlopen()s version-specific libraries that
 * export these symbols, allowing runtime PHP version switching.
 *
 * Each PHP version is compiled with this ABI layer, producing a
 * libphpx-X.Y.so that can be loaded dynamically.
 */

#ifndef PHPX_ABI_H
#define PHPX_ABI_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* ABI version - increment when breaking changes are made */
#define PHPX_ABI_VERSION 1

/* ============================================================================
 * Version Information
 * ============================================================================ */

typedef struct {
    int abi_version;        /* PHPX_ABI_VERSION */
    const char *php_version;    /* e.g., "8.3.15" */
    int php_version_id;     /* e.g., 80315 */
    const char *zend_version;   /* Zend Engine version */
    int is_debug;           /* Built with debug mode */
    int is_zts;             /* Built with thread safety */

    /* Library versions (may be NULL if not available) */
    const char *icu_version;
    const char *libxml_version;
    const char *openssl_version;
    const char *pcre_version;
    const char *zlib_version;
    const char *curl_version;
} phpx_version_info;

/* Get version information about this runtime */
typedef const phpx_version_info* (*phpx_get_version_info_fn)(void);

/* ============================================================================
 * CLI Mode Operations
 * ============================================================================ */

/* Set INI entries before initialization (newline-separated key=value pairs) */
typedef void (*phpx_set_ini_entries_fn)(const char *entries);

/* Execute a PHP script file, returns exit code */
typedef int (*phpx_execute_script_fn)(const char *script_path, int argc, char **argv);

/* Execute PHP code directly, returns exit code */
typedef int (*phpx_execute_code_fn)(const char *code, int argc, char **argv);

/* Syntax check (lint) a PHP file, returns 0 if valid */
typedef int (*phpx_lint_file_fn)(const char *script_path, int argc, char **argv);

/* Print phpinfo() output, flag=-1 for all info */
typedef int (*phpx_info_fn)(int flag, int argc, char **argv);

/* Print loaded PHP modules */
typedef int (*phpx_print_modules_fn)(int argc, char **argv);

/* Get loaded extensions as newline-separated string (caller must free) */
typedef char* (*phpx_get_loaded_extensions_fn)(int argc, char **argv);

/* Free a string allocated by phpx functions */
typedef void (*phpx_free_string_fn)(char *str);

/* ============================================================================
 * Web/Server Mode Operations
 * ============================================================================ */

/* Request context - passed between Rust and C */
typedef struct {
    /* Request info */
    const char *method;
    const char *uri;
    const char *query_string;
    const char *content_type;
    size_t content_length;
    const char *request_body;
    size_t request_body_len;
    size_t request_body_read;

    /* Headers (key: value pairs, newline separated) */
    const char *headers;

    /* Document root and script */
    const char *document_root;
    const char *script_filename;

    /* Server info */
    const char *server_name;
    int server_port;
    const char *remote_addr;
    int remote_port;

    /* Response output buffer (filled by PHP) */
    char *response_body;
    size_t response_body_len;
    size_t response_body_cap;

    /* Response headers (filled by PHP) */
    char *response_headers;
    size_t response_headers_len;
    size_t response_headers_cap;

    /* Response status */
    int response_status;
} phpx_request_context;

/* Initialize web SAPI (call once at server startup) */
typedef int (*phpx_web_init_fn)(void);

/* Shutdown web SAPI (call once at server shutdown) */
typedef void (*phpx_web_shutdown_fn)(void);

/* Execute a web request */
typedef int (*phpx_web_execute_fn)(phpx_request_context *ctx);

/* Free response buffers in request context */
typedef void (*phpx_free_response_fn)(phpx_request_context *ctx);

/* ============================================================================
 * Worker Mode Operations
 * ============================================================================ */

/* Global initialization for worker mode (call from main thread) */
typedef int (*phpx_worker_global_init_fn)(void);

/* Run a worker script */
typedef int (*phpx_worker_run_fn)(const char *script_filename, const char *document_root);

/* Set pending request for worker */
typedef void (*phpx_worker_set_request_fn)(phpx_request_context *ctx);

/* ============================================================================
 * Function Table
 *
 * This structure contains pointers to all exported functions.
 * The main binary loads this table from the dynamic library.
 * ============================================================================ */

typedef struct {
    /* Version info */
    phpx_get_version_info_fn get_version_info;

    /* CLI mode */
    phpx_set_ini_entries_fn set_ini_entries;
    phpx_execute_script_fn execute_script;
    phpx_execute_code_fn execute_code;
    phpx_lint_file_fn lint_file;
    phpx_info_fn info;
    phpx_print_modules_fn print_modules;
    phpx_get_loaded_extensions_fn get_loaded_extensions;
    phpx_free_string_fn free_string;

    /* Web mode */
    phpx_web_init_fn web_init;
    phpx_web_shutdown_fn web_shutdown;
    phpx_web_execute_fn web_execute;
    phpx_free_response_fn free_response;

    /* Worker mode */
    phpx_worker_global_init_fn worker_global_init;
    phpx_worker_run_fn worker_run;
    phpx_worker_set_request_fn worker_set_request;
} phpx_function_table;

/* Every libphpx.so must export this function */
typedef const phpx_function_table* (*phpx_get_function_table_fn)(void);

/* Symbol name for the entry point */
#define PHPX_ENTRY_SYMBOL "phpx_get_function_table"

#ifdef __cplusplus
}
#endif

#endif /* PHPX_ABI_H */
