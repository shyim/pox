/*
 * phpx - PHP CLI embedded in Rust
 * Header file for FFI bindings
 */

#ifndef PHPX_H
#define PHPX_H

#ifdef __cplusplus
extern "C" {
#endif

/*
 * Execute a PHP script file.
 * Returns the exit status code.
 */
int phpx_execute_script(const char *script_path, int argc, char **argv);

/*
 * Execute PHP code passed as a string (like php -r).
 * Returns the exit status code.
 */
int phpx_execute_code(const char *code, int argc, char **argv);

/*
 * Get PHP version string.
 */
const char *phpx_get_version(void);

/*
 * Get PHP version ID (e.g., 80300 for PHP 8.3.0).
 */
int phpx_get_version_id(void);

/*
 * Get Zend Engine version string.
 */
const char *phpx_get_zend_version(void);

/*
 * Check if PHP is built with debug mode.
 */
int phpx_is_debug(void);

/*
 * Check if PHP is built with ZTS (thread safety).
 */
int phpx_is_zts(void);

/*
 * Get ICU version (from intl extension). Returns NULL if not available.
 */
const char *phpx_get_icu_version(void);

/*
 * Get libxml version. Returns NULL if not available.
 */
const char *phpx_get_libxml_version(void);

/*
 * Get OpenSSL version text. Returns NULL if not available.
 */
const char *phpx_get_openssl_version(void);

/*
 * Get PCRE version. Returns NULL if not available.
 */
const char *phpx_get_pcre_version(void);

/*
 * Get zlib version. Returns NULL if not available.
 */
const char *phpx_get_zlib_version(void);

/*
 * Get curl version. Returns NULL if not available.
 */
const char *phpx_get_curl_version(void);

#ifdef __cplusplus
}
#endif

#endif /* PHPX_H */
