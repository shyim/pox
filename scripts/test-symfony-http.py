#!/usr/bin/env python3
"""Exercise a copied, installed Symfony Demo application through Pox HTTP."""
import argparse
import concurrent.futures
import hashlib
import http.client
import http.cookies
import html
import re
import urllib.parse
import json
import os
from pathlib import Path
import shutil
import socket
import subprocess
import tempfile
import time
import xml.etree.ElementTree as ET


def digest(path):
    with path.open('rb') as file:
        return hashlib.file_digest(file, 'sha256').hexdigest()


def run(args, worker):
    with tempfile.TemporaryDirectory(prefix='pox-symfony-') as temporary:
        root = Path(temporary) / 'app'
        shutil.copytree(args.application, root, ignore=shutil.ignore_patterns('.git', 'var', 'node_modules'))
        runtime_loader = root / 'vendor/autoload_runtime.php'
        if not runtime_loader.exists():
            template = (root / 'vendor/symfony/runtime/Internal/autoload_runtime.template').read_text()
            runtime_loader.write_text(template.replace('%runtime_class%', repr('Symfony\\Component\\Runtime\\SymfonyRuntime')).replace('%runtime_options%', "['project_dir' => dirname(__DIR__)]"))
        environment = dict(os.environ, POX_PHP_RUNTIME=str(args.runtime), APP_ENV='prod', APP_DEBUG='0',
                           APP_SECRET='pox-isolated-integration-fixture',
                           DATABASE_URL='sqlite:///' + str(root / 'data/database.sqlite'), MAILER_DSN='null://null')
        log_path = Path(temporary) / 'pox.log'
        with log_path.open('wb') as log:
            for command in ['importmap:install', 'cache:warmup', 'asset-map:compile']:
                for attempt in range(3 if command == 'importmap:install' else 1):
                    result = subprocess.run([str(args.binary), 'bin/console', command, '--env=prod', '--no-debug', '--no-interaction'],
                                            cwd=root, env=environment, stdout=log, stderr=log, timeout=120)
                    if result.returncode == 0:
                        break
                if result.returncode:
                    print(log_path.read_text(errors='replace')[-6000:])
                    result.check_returncode()
        bootstrap = """<?php
require dirname(__DIR__).'/vendor/autoload.php';
(new Symfony\\Component\\Dotenv\\Dotenv())->bootEnv(dirname(__DIR__).'/.env');
$kernel = new App\\Kernel('prod', false);
$handle = static function () use ($kernel) {
    $request = Symfony\\Component\\HttpFoundation\\Request::createFromGlobals();
    $response = $kernel->handle($request);
    $response->send();
    $kernel->terminate($request, $response);
};
"""
        (root / 'public/index.php').write_text(bootstrap + ('while (pox_handle_request($handle)) {}' if worker else '$handle();'))
        (root / 'pox.toml').write_text('[server.limits]\nworker_max_requests = 20\nrequest_timeout_ms = 10000\n')
        with socket.socket() as reservation:
            reservation.bind(('127.0.0.1', 0))
            port = reservation.getsockname()[1]
        command = [str(args.binary), 'server', '--host', '127.0.0.1', '--port', str(port),
                   '--document-root', str(root / 'public'), '--workers', '2']
        if worker:
            command += ['--worker', str(root / 'public/index.php')]
        with log_path.open('ab') as log:
            process = subprocess.Popen(command, cwd=root, env=environment, stdout=log, stderr=log)
        def request(path, method='GET', body=None, headers=None):
            connection = http.client.HTTPConnection('127.0.0.1', port, timeout=15)
            try:
                connection.request(method, path, body=body, headers=headers or {})
                response = connection.getresponse()
                headers = dict(response.getheaders())
                headers['set-cookie-list'] = [value for name, value in response.getheaders() if name.lower() == 'set-cookie']
                return response.status, headers, response.read()
            finally:
                connection.close()
        try:
            deadline = time.monotonic() + 15
            while True:
                assert process.poll() is None, log_path.read_text(errors='replace')[-6000:]
                try:
                    with socket.create_connection(('127.0.0.1', port), timeout=0.1):
                        break
                except OSError:
                    assert time.monotonic() < deadline, 'Symfony server did not listen'
                    time.sleep(0.02)
            status, headers, body = request('/en/blog/')
            assert status == 200 and b'<html' in body and len(body) > 1000, (status, body[:500])
            status, headers, body = request('/en/blog/rss.xml')
            assert status == 200 and ET.fromstring(body).tag == 'rss', (status, body[:500])
            assert len(ET.fromstring(body).findall('./channel/item')) > 0
            status, headers, body = request('/en/login')
            assert status == 200 and b'name="_csrf_token"' in body, (status, body[:500])
            tokens = re.findall(rb'name="_csrf_token"[^>]*value="([^"]+)"', body)
            assert len(tokens) == 1
            jar = http.cookies.SimpleCookie()
            for value in headers['set-cookie-list']:
                jar.load(value)
            cookie = '; '.join(morsel.OutputString(attrs=[]) for morsel in jar.values())
            form = urllib.parse.urlencode({'_username': 'jane_admin', '_password': 'kitten',
                                           '_csrf_token': html.unescape(tokens[0].decode()),
                                           '_target_path': '/en/admin/post/'})
            status, headers, body = request('/en/login', 'POST', form,
                {'Content-Type': 'application/x-www-form-urlencoded', 'Cookie': cookie,
                 'Origin': f'http://127.0.0.1:{port}', 'Referer': f'http://127.0.0.1:{port}/en/login'})
            assert status in (302, 303) and headers.get('location', '').endswith('/en/admin/post/'), 'demo fixture login failed'
            for value in headers['set-cookie-list']:
                jar.load(value)
            cookie = '; '.join(morsel.OutputString(attrs=[]) for morsel in jar.values())
            assert cookie, 'login did not establish a session'
            for _ in range(6):
                assert request('/en/admin/post/', headers={'Cookie': cookie})[0] == 200, 'session was not retained'
                assert request('/en/admin/post/')[0] in (302, 303), 'authentication leaked to an anonymous request'
            def parallel(number):
                path = '/en/blog/' if number % 2 == 0 else '/fr/blog/'
                status, headers, body = request(path)
                assert status == 200 and ('lang="en"' if number % 2 == 0 else 'lang="fr"').encode() in body, (status, body[:100])
            with concurrent.futures.ThreadPoolExecutor(max_workers=8) as pool:
                list(pool.map(parallel, range(80)))
            process.terminate()
            assert process.wait(timeout=35) == 0
            return {'mode': 'worker' if worker else 'standard', 'passed': True,
                    'checks': ['Twig blog render with Doctrine SQLite data', 'RSS XML with database posts',
                               'login form CSRF generation', 'fixture login and session continuity',
                               'anonymous requests do not inherit authentication', '80 concurrent alternating-locale requests',
                               'graceful shutdown'],
                    'worker_recycle_limit': 20, 'log_tail': log_path.read_text(errors='replace').splitlines()[-3:]}
        except Exception:
            print(log_path.read_text(errors='replace')[-6000:])
            raise
        finally:
            if process.poll() is None:
                process.kill()
                process.wait()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--application', type=Path, required=True)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--runtime', type=Path, required=True)
    args = parser.parse_args()
    for key in ('application', 'binary', 'runtime'):
        setattr(args, key, getattr(args, key).resolve(strict=True))
    assert (args.application / 'vendor/autoload.php').is_file()
    provenance = {'binary_sha256': digest(args.binary), 'runtime_sha256': digest(args.runtime),
                  'composer_lock_sha256': digest(args.application / 'composer.lock'),
                  'installed_packages_sha256': digest(args.application / 'vendor/composer/installed.json')}
    installed = json.loads((args.application / 'vendor/composer/installed.json').read_text())
    provenance['framework_packages'] = {p['name']: p['version'] for p in installed['packages']
                                      if p['name'] in ('symfony/framework-bundle', 'symfony/http-kernel', 'doctrine/orm', 'twig/twig')}
    print(json.dumps({'provenance': provenance, 'results': [run(args, worker) for worker in (False, True)]}))


if __name__ == '__main__':
    main()
