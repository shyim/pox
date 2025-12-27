/*
 * phpx ABI Export Layer
 *
 * This file provides the stable ABI interface for dynamically loaded
 * PHP runtime libraries. It wraps the internal embed.c functions and
 * exports them through a function table.
 *
 * Build this file alongside embed.c to create libphpx.so for each PHP version.
 */

#include "abi.h"
#include <string.h>

/* Forward declarations for embed.c functions */
extern void phpx_set_ini_entries(const char *entries);
extern int phpx_execute_script(const char *script_path, int argc, char **argv);
extern int phpx_execute_code(const char *code, int argc, char **argv);
extern int phpx_lint_file(const char *script_path, int argc, char **argv);
extern int phpx_info(int flag, int argc, char **argv);
extern int phpx_print_modules(int argc, char **argv);
extern char *phpx_get_loaded_extensions(int argc, char **argv);
extern void phpx_free_string(char *str);
extern const char *phpx_get_version(void);
extern int phpx_get_version_id(void);
extern const char *phpx_get_zend_version(void);
extern int phpx_is_debug(void);
extern int phpx_is_zts(void);
extern const char *phpx_get_icu_version(void);
extern const char *phpx_get_libxml_version(void);
extern const char *phpx_get_openssl_version(void);
extern const char *phpx_get_pcre_version(void);
extern const char *phpx_get_zlib_version(void);
extern const char *phpx_get_curl_version(void);
extern int phpx_web_init(void);
extern void phpx_web_shutdown(void);
extern int phpx_web_execute(phpx_request_context *ctx);
extern void phpx_free_response(phpx_request_context *ctx);
extern int phpx_worker_global_init(void);
extern int phpx_worker_run(const char *script_filename, const char *document_root);
extern void phpx_worker_set_request(phpx_request_context *ctx);

/* Static version info structure */
static phpx_version_info version_info = {0};
static int version_info_initialized = 0;

/* Initialize version info lazily */
static void init_version_info(void) {
    if (version_info_initialized) return;

    version_info.abi_version = PHPX_ABI_VERSION;
    version_info.php_version = phpx_get_version();
    version_info.php_version_id = phpx_get_version_id();
    version_info.zend_version = phpx_get_zend_version();
    version_info.is_debug = phpx_is_debug();
    version_info.is_zts = phpx_is_zts();
    version_info.icu_version = phpx_get_icu_version();
    version_info.libxml_version = phpx_get_libxml_version();
    version_info.openssl_version = phpx_get_openssl_version();
    version_info.pcre_version = phpx_get_pcre_version();
    version_info.zlib_version = phpx_get_zlib_version();
    version_info.curl_version = phpx_get_curl_version();

    version_info_initialized = 1;
}

/* ABI wrapper: get version info */
static const phpx_version_info* abi_get_version_info(void) {
    init_version_info();
    return &version_info;
}

/* ABI wrapper: set INI entries */
static void abi_set_ini_entries(const char *entries) {
    phpx_set_ini_entries(entries);
}

/* ABI wrapper: execute script */
static int abi_execute_script(const char *script_path, int argc, char **argv) {
    return phpx_execute_script(script_path, argc, argv);
}

/* ABI wrapper: execute code */
static int abi_execute_code(const char *code, int argc, char **argv) {
    return phpx_execute_code(code, argc, argv);
}

/* ABI wrapper: lint file */
static int abi_lint_file(const char *script_path, int argc, char **argv) {
    return phpx_lint_file(script_path, argc, argv);
}

/* ABI wrapper: phpinfo */
static int abi_info(int flag, int argc, char **argv) {
    return phpx_info(flag, argc, argv);
}

/* ABI wrapper: print modules */
static int abi_print_modules(int argc, char **argv) {
    return phpx_print_modules(argc, argv);
}

/* ABI wrapper: get loaded extensions */
static char* abi_get_loaded_extensions(int argc, char **argv) {
    return phpx_get_loaded_extensions(argc, argv);
}

/* ABI wrapper: free string */
static void abi_free_string(char *str) {
    phpx_free_string(str);
}

/* ABI wrapper: web init */
static int abi_web_init(void) {
    return phpx_web_init();
}

/* ABI wrapper: web shutdown */
static void abi_web_shutdown(void) {
    phpx_web_shutdown();
}

/* ABI wrapper: web execute */
static int abi_web_execute(phpx_request_context *ctx) {
    return phpx_web_execute(ctx);
}

/* ABI wrapper: free response */
static void abi_free_response(phpx_request_context *ctx) {
    phpx_free_response(ctx);
}

/* ABI wrapper: worker global init */
static int abi_worker_global_init(void) {
    return phpx_worker_global_init();
}

/* ABI wrapper: worker run */
static int abi_worker_run(const char *script_filename, const char *document_root) {
    return phpx_worker_run(script_filename, document_root);
}

/* ABI wrapper: worker set request */
static void abi_worker_set_request(phpx_request_context *ctx) {
    phpx_worker_set_request(ctx);
}

/* Static function table */
static const phpx_function_table function_table = {
    /* Version info */
    .get_version_info = abi_get_version_info,

    /* CLI mode */
    .set_ini_entries = abi_set_ini_entries,
    .execute_script = abi_execute_script,
    .execute_code = abi_execute_code,
    .lint_file = abi_lint_file,
    .info = abi_info,
    .print_modules = abi_print_modules,
    .get_loaded_extensions = abi_get_loaded_extensions,
    .free_string = abi_free_string,

    /* Web mode */
    .web_init = abi_web_init,
    .web_shutdown = abi_web_shutdown,
    .web_execute = abi_web_execute,
    .free_response = abi_free_response,

    /* Worker mode */
    .worker_global_init = abi_worker_global_init,
    .worker_run = abi_worker_run,
    .worker_set_request = abi_worker_set_request,
};

/*
 * Entry point exported from the shared library.
 * The main phpx binary calls this to get the function table.
 */
__attribute__((visibility("default")))
const phpx_function_table* phpx_get_function_table(void) {
    return &function_table;
}
