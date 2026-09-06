/* Test-only extension: reject module startup before any HTTP listener opens. */
#include <php.h>

PHP_MINIT_FUNCTION(pox_test_startup_failure)
{
    return FAILURE;
}

zend_module_entry pox_test_startup_failure_module_entry = {
    STANDARD_MODULE_HEADER,
    "pox_test_startup_failure", NULL,
    PHP_MINIT(pox_test_startup_failure), NULL, NULL, NULL, NULL,
    "0.0.0-test-only", STANDARD_MODULE_PROPERTIES
};

ZEND_GET_MODULE(pox_test_startup_failure)
