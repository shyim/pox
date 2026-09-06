/* Test-only extension: never install in an application runtime.
 * A protected-page write generates a synchronous native memory fault, rather
 * than a process-directed signal that Zend may handle differently. */
#include <php.h>
#include <stdlib.h>
#include <sys/mman.h>
#include <unistd.h>

ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_pox_test_native_fault, 0, 0, IS_VOID, 0)
ZEND_END_ARG_INFO()

PHP_FUNCTION(pox_test_native_fault)
{
    /* Avoid PHP API symbol imports: the embedded runtime exposes its ABI table
     * only, and musl resolves extension imports eagerly. Setup failures abort
     * with SIGABRT so they cannot be mistaken for the expected SIGSEGV. */
    if (ZEND_NUM_ARGS() != 0) {
        abort();
    }
    long size = sysconf(_SC_PAGESIZE);
    if (size <= 0) {
        abort();
    }
    void *page = mmap(NULL, (size_t)size, PROT_NONE,
                      MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (page == MAP_FAILED) {
        abort();
    }
    *(volatile unsigned char *)page = 1;
    munmap(page, (size_t)size);
    abort();
}

static const zend_function_entry functions[] = {
    PHP_FE(pox_test_native_fault, arginfo_pox_test_native_fault)
    PHP_FE_END
};

zend_module_entry pox_test_native_fault_module_entry = {
    STANDARD_MODULE_HEADER,
    "pox_test_native_fault", functions,
    NULL, NULL, NULL, NULL, NULL,
    "0.0.0-test-only", STANDARD_MODULE_PROPERTIES
};

ZEND_GET_MODULE(pox_test_native_fault)
