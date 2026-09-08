# Symfony application validation

The [Symfony harness](../scripts/test-symfony-http.py) exercises an installed
Symfony Demo application copied into a temporary directory. It uses the copied
SQLite fixture database and local application cache, and leaves the source
checkout unchanged. Provide a demo checkout with its Composer dependencies and
fixture database already installed:

```bash
python3 scripts/test-symfony-http.py \
  --application /absolute/path/to/symfony-demo \
  --binary target/debug/pox \
  --runtime /absolute/path/to/libpox_php.so
```

Preparation regenerates a missing Runtime loader from the installed Symfony
template, installs importmap assets, warms the production cache and compiles the
asset map. Asset/icon preparation can access public CDNs; importmap installation
has up to three attempts for transient download errors. HTTP assertions are not
retried. Preparation never disables certificate verification. The generated HTTP
entrypoint uses the actual application Kernel, Request::createFromGlobals,
Response::send and Kernel::terminate. Worker mode retains the Kernel across the
Pox request loop and relies on Symfony's per-main-request service reset.

Both modes verify Twig blog rendering with Doctrine/SQLite data, a populated RSS
feed, login-form CSRF generation, fixture-account login and session continuity,
anonymous requests remaining unauthenticated, 80 concurrent alternating English
and French requests, and successful graceful shutdown. Worker recycling is
configured at 20 requests. The fixture account is the demo's public `jane_admin`
account; credentials and session cookies are not written to the result artifact.
The copied application uses its own SQLite database and a null mail transport.

The [recorded Linux result](https://github.com/shyim/pox/blob/2c777c4f0fadb8da15f16498dcc3331945a6a6fb/docs/validation/http-symfony-linux-2026-09-06.json) passes
all seven checks per mode on PHP 8.5.9 ZTS, Symfony FrameworkBundle/HttpKernel
8.1.0, Doctrine ORM 3.6.7 and Twig 3.27.1. It includes binary/runtime and dependency
manifest hashes. This is an HTTP integration test, not a browser UI test or a
claim that every Symfony bundle or deployment configuration is supported.


The same seven checks also pass in both modes on PHP 8.4.25 ZTS; see the
[PHP 8.4 application result](https://github.com/shyim/pox/blob/2c777c4f0fadb8da15f16498dcc3331945a6a6fb/docs/validation/http-symfony-php84-linux-2026-09-06.json).
It uses the same dependency manifests and HTTP harness. The locally built PHP 8.4
runtime links dynamic libstdc++ and is validation-only, as described in the
[PHP 8.4 runtime evidence](https://github.com/shyim/pox/blob/2c777c4f0fadb8da15f16498dcc3331945a6a6fb/docs/validation/http-php84-runtime-linux-2026-09-06.json).
These short application runs do not measure long-term framework memory growth.
